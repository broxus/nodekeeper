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
