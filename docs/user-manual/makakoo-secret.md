# `makakoo secret`

Store API keys and other secrets in the OS keyring (macOS Keychain / Linux Secret Service / Windows Credential Manager) instead of in a dotfile.

## Example

```sh
makakoo secret set AIL_API_KEY
makakoo secret get AIL_API_KEY
makakoo secret list
makakoo secret delete AIL_API_KEY
```

## Full command surface

Run with `--help` for every flag, subcommand, and exit code:

```sh
makakoo secret --help
```

See also: [User manual index](index.md), [Use cases](../use-cases.md), [Troubleshooting](../troubleshooting/index.md).
