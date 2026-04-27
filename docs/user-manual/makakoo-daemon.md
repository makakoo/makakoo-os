# `makakoo daemon`

Control the Makakoo background daemon (LaunchAgent on macOS, systemd unit on Linux, auto-launch on Windows). Automatically installed by `makakoo install`.

## Example

```sh
makakoo daemon status
makakoo daemon install
makakoo daemon uninstall
makakoo daemon logs --tail 50
```

## Full command surface

Run with `--help` for every flag, subcommand, and exit code:

```sh
makakoo daemon --help
```

See also: [User manual index](index.md), [Use cases](../use-cases.md), [Troubleshooting](../troubleshooting/index.md).
