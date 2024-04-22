# 0.2.16 (2024-04-22)

### Added

- Added support for the new default repo and renamed node binary.
- Added `getrawstats` subcommand to get validation stats as is.

# 0.2.15 (2024-02-19)

### Fixed

- Fixed incorrect memory info parsing (https://github.com/GuillaumeGomez/sysinfo/pull/1058).

# 0.2.14 (2024-02-13)

### Changed

- Updated the default node config.

# 0.2.12 / 0.2.13 (2024-02-10)

### Changed

- Build deb package with the lowest version of glibc (2.27).

# 0.2.11 (2024-01-28)

### Fixed

- Added additional retries for `GetCapabilities` query to avoid false negative node availability checks.

# 0.2.10 (2023-10-17)

### Changed

- Updated default global config for Everscale.

# 0.2.9 (2023-08-29)

### Fixed

- Fixed potential units mismatch in storage fee computation.

# 0.2.8 (2023-08-29)

### Added

- Added `storage_fee` field for `nodekeeper validator balance` output entries.

### Changed

- When maintaining DePool balances, the accumulated storage fee on proxies
  is now taken into account.

# 0.2.7 (2023-08-24)

### Added

- Added `nodekeeper validator balance` subcommand which outputs a structured info about
  validator wallet(s) and address(es).
- Added `nodekeeper validator withdraw <dest> <amount>` subcommand which allows to easily
  withdraw tokens from the validator wallet.

### Changed

- Validator manager subcommand moved from `nodekeeper validator` to `nodekeeper validator run`.

### Fixed

- Fixed hidden cursor state after `ctrl+C` interruption in prompts.

# 0.2.6 (2023-07-09)

### Changed

- The `validator` service will not be restarted during an update.

### Fixed

- Fixed file path autocompletion and `~/` now works as in shell.

# 0.2.5 (2023-07-06)

### Added

- Added `validator-exporter` systemd service for metrics.

  It listens on port `10000` by default. You can override it with:
  ```bash
  sudo systemctl edit validator-exporter
  ```
  ```ini
  [Service]
  Environment=PORT=10000
  Environment=INTERVAL=10
  ```

# 0.2.4 (2023-04-05)

### Added

- Added support for a new Venom update.
- Added a `force` flag to the `validator` command. Used to force elect without checking the network config.

### Fixed

- Double check election id before adding validator keys.

# 0.2.3 (2023-04-05)

### Added

- JSON output for template initialization from non-tty environment.

### Changed

- Intermediate messages are now printed to the `stderr`.

# 0.2.2 (2023-03-27)

### Fixed

- Fixed `statvfs` for the newly created node.

# 0.2.1 (2023-03-27)

### Added

- Show system info during `init`.
- Detect default node repo and features based on global config.

# 0.2.0 (2023-03-24)

### Added

- Added support for the new stEVER flow.
- Detect currency based on global config.

### Changed

- Renamed tool to `nodekeeper`.
- Renamed `systemd` services to `validator` and `validator-manager`.

# 0.1.5 (2023-03-16)

### Added

- `.deb` package build.
- You can now specify file path for a global config during `stever node init`
  (it used to be only URL).
- Added check for the Rust installation.

# 0.1.4 (2023-02-15)

### Added

- Extended exported metrics.
    * Added `sync_status` label to the `node_ready` metric.
    * `validation_enabled`: `0`/`1`.
    * <sub>if validation is enabled</sub>

      `validator_type`: `0` - single / `1` - depool.
    * <sub>if validation is enabled and `validator_type=0`</sub>

      `validator_single_stake_per_round`: stake in nano EVERs.

      Labels: `validator` - validator wallet address.
    * <sub>if validation is enabled and `validator_type=1`</sub>

      `validator_depool_type`: `0` - default_v3, `1` - stever_v1, `2` - stever_v2.

      Labels: `validator` - validator wallet address, `depool` - depool address.

### Changed

- Refactored project structure.

# 0.1.3 (2023-02-06)

### Added

- Added support for initialization templates. Templates can be specified for `stever init` command or
  its subcommands (except `systemd`). They are mostly used for running stever from scripts (i.e. from ansible).

  See [example.toml](/templates/example.toml) for more details.

- Added `--user`,`--enable` and `--start` params to the `stever init systemd` to allow using it from scripts.

- Added `stever node gendht` to export signed global config entries.

### Changed

- Separate `stever init systemd` is now always required after the first initialization.

# 0.1.2 (2023-02-03)

### Added

- Added support for signature id. Signature for networks with this capability enabled will now be
  calculated differently to prevent security issues.
- Added support for cloning the specific branch in repo and build the node with specified features.
  While initializing the node with `stever init`, add these flags after the repo url:
    - `-b,--branch <branch>` - branch name;
    - `-f,--features <feature>...` - list of features for `cargo build`;

# 0.1.1 (2023-01-27)

### Added

- Added support for the new version of the stEver DePool contract (`depool_type = "stever_v2"`).
- Added `--version/-v` flag to get application version.
- Added random offset from the beginning of the elections to spread the load (`0..1/4` of elections range).
  > Could be disabled by adding a flag `--disable-random-shift`
- DePool and proxy balances are now replenished if there are not enough funds on them.

### Changed

- `stever init --rebuild` now always replaces the existing node binary even if it is running (behavior is similar to `cp -f`).
- Updated the default node config (added the `"gc": { .. }` section).

### Fixed

- Fixed races in the blocks subscription loop.

# 0.1.0 (2022-12-20)

Initial release.
