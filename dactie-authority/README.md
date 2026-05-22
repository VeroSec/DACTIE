# DACTIE-Authority
This package contains the implementation of the authority peer. The authority should be started first and can be used as
a bootstrap peer for other peers. Other peers need it to get their group member key. This key is need for publishing message or verifying the messages received.

New peers are automatically accepted at the moment.

## Startup
```
Usage: dactie-authority [OPTIONS] --km-dir <KM_DIR>
Example: dactie-authority --km-dir ./key_material

Options:
  -K, --km-dir <KM_DIR>        Location of the folder, where the keymaterial should be saved
  -p, --port <PORT>            Port to run service on
  -l, --load-file <LOAD_FILE>  Load_file, for stored groups and subscribed topics
  -h, --help                   Print help
  -V, --version                Print version

```

## Commands
The authority takes the following commands:
```
- req_open <signatur>: Requests open pairings from archives
- open <signatur>: Opens Signatur   
- end_init_archive: Ends Init Phase no new archives can join

```
## Initialzation phase
The authority starts in the initialization mode. In this mode it accepts new archive connects. The archives are necessary for the groupmember-key-creation.
When all archives are registered it can be switched to the normal mode of operation with `end_init_archive`.

## Signature Opening
The authority can open a signature if it receives the corresponding pairings from the archives. In this demo, the archives always create the pairings when request.
In a real world scenario they have the freedom of decision.

1. The authority requests the pairings from the archives with `req_open <signature>`
2. When the authority has received enough pairings to achieve the necessary threshold (num_archives/2) it can open the signature with `open <signature>`