use std::time::Duration;

use anyhow::Result;
use argh::FromArgs;
use tokio_util::sync::CancellationToken;

use super::CliContext;
use crate::validator::{ValidationManager, ValidationParams};

#[derive(FromArgs)]
/// Validation manager service
#[argh(subcommand, name = "validator")]
pub struct Cmd {
    /// max timediff (in seconds). 120 seconds default
    #[argh(option, default = "120")]
    max_time_diff: u16,

    /// offset after stake unfreeze (in seconds). 600 seconds default
    #[argh(option, default = "600")]
    stake_unfreeze_offset: u32,

    /// elections start offset (in seconds). 600 seconds default
    #[argh(option, default = "600")]
    elections_start_offset: u32,

    /// elections end offset (in seconds). 120 seconds default
    #[argh(option, default = "120")]
    elections_end_offset: u32,

    /// min retry interval (in seconds). 10 seconds default
    #[argh(option, default = "10")]
    min_retry_interval: u64,

    /// max retry interval (in seconds). 300 seconds default
    #[argh(option, default = "300")]
    max_retry_interval: u64,

    /// interval increase factor. 2.0 times default
    #[argh(option, default = "2.0")]
    retry_interval_multiplier: f64,

    /// forces stakes to be sent right after the start of each election
    #[argh(switch)]
    disable_random_shift: bool,

    /// ignore contracts deployment
    #[argh(switch)]
    ignore_deploy: bool,
}

impl Cmd {
    pub async fn run(mut self, ctx: CliContext) -> Result<()> {
        // Start listening termination signals
        let signal_rx = broxus_util::any_signal(broxus_util::TERMINATION_SIGNALS);

        // Create validation manager
        let mut manager = ValidationManager::new(
            ctx.dirs,
            ValidationParams {
                max_time_diff: std::cmp::max(self.max_time_diff as i32, 5),
                stake_unfreeze_offset: self.stake_unfreeze_offset,
                elections_start_offset: self.elections_start_offset,
                elections_end_offset: self.elections_end_offset,
                disable_random_shift: self.disable_random_shift,
                ignore_deploy: self.ignore_deploy,
            },
        );

        // Spawn cancellation future
        let cancellation_token = CancellationToken::new();
        let cancelled = cancellation_token.cancelled();

        tokio::spawn({
            let guard = manager.guard().clone();
            let cancellation_token = cancellation_token.clone();

            async move {
                if let Ok(signal) = signal_rx.await {
                    tracing::warn!(?signal, "received termination signal");
                    let _guard = guard.lock().await;
                    cancellation_token.cancel();
                }
            }
        });

        // Prepare validation future
        let validation_fut = async {
            self.min_retry_interval = std::cmp::max(self.min_retry_interval, 1);
            self.max_retry_interval =
                std::cmp::max(self.max_retry_interval, self.min_retry_interval);
            self.retry_interval_multiplier = num::Float::max(self.retry_interval_multiplier, 1.0);

            let mut interval = self.min_retry_interval;
            loop {
                if let Err(e) = manager.try_validate().await {
                    tracing::error!("error occurred: {e:?}");
                }

                tracing::info!("retrying in {interval} seconds");
                tokio::time::sleep(Duration::from_secs(interval)).await;

                interval = std::cmp::min(
                    self.max_retry_interval,
                    (interval as f64 * self.retry_interval_multiplier) as u64,
                );
            }
        };

        // Cancellable main loop
        tokio::select! {
            _ = validation_fut => {},
            _ = cancelled => {},
        };

        Ok(())
    }
}
