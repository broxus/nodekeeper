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
