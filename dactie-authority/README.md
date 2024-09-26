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
open <signatur>: Opens Signatur
```

## Future Work
This proof of concept is missing the Crypto Implementation of the Threshold Group Signature scheme mentioned in the Thesis.
Therefore the Authority can add new members and open signatures. In future work that has access of a group signatures implementation, 
the opening function should be moved to the archives.