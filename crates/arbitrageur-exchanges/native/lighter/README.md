# Vendored Lighter signer

`liblighter_signer.so` is a C-shared build of the upstream Go package
`github.com/elliottech/lighter-go/sharedlib`. It is used only for local Lighter
transaction signing; the Rust adapter still performs nonce lookup and HTTP
`sendTx` submission separately.

## Verified binary identity

- File: `liblighter_signer.so`
- SHA-256: `8cb2fbcadab0b9b3fed4a524998b825818b677fcf9c49bc1f18b4ca02a233753`
- Embedded module metadata observed in the binary:
  - `github.com/elliottech/lighter-go` `v1.0.10`
  - Go toolchain `<go1.24.6`
- Header: `liblighter_signer.h`

The Rust loader hashes the file before `dlopen`. A digest mismatch is a hard
error and never falls back to another candidate. CI repeats the same digest
check in `.github/workflows/ci.yml`.

## Override policy

`LIGHTER_SIGNER_LIB` may point to a custom signer only when
`LIGHTER_SIGNER_SHA256` is also set to that file's exact lowercase or uppercase
64-hex SHA-256. Overrides fail closed: no fallback to the sidecar or vendored
binary is attempted after an override failure.

## Reproduction status

The checked-in binary currently has integrity verification but not a fully
reproduced build script in this repository. To rebuild from source, pin the
`lighter-go` release matching the embedded module version, build the
`sharedlib` package as `c-shared` with the same Go toolchain family, compare
the exported symbols against `liblighter_signer.h`, and only replace the
vendored binary after explicitly recording and reviewing the new SHA-256. Do
not treat the checksum as provenance: it prevents silent drift, while the
source-pinned reproduction recipe remains an open supply-chain task.
