# DACTIE-Archive
This package contains the implementation of the archive peer. 

## Storage
To start an archive a PostgresDB is needed. It is possible to set an external one up or use the one that comes with the docker compose file.
The docker database can be build and started with `docker compose up db`

## Startup
```
Usage: dactie-archive [OPTIONS] --km-dir <KM_DIR> --url <URL> --aa-id <AA_ID> --identity <IDENTITY>
Example: dactie-archive -K ./key_material/ -u "postgres://archive:archivepw@localhost/archivedb" -i archive1 -a 12D3KooWKeDvTggGdBuRjiUnECtJTgC5tjJYMaQ4Q9BGWimsoJjy -B /ip4/127.0.0.1/tcp/10001

Options:
  -K, --km-dir <KM_DIR>      Location of the folder, where the keymaterial should be saved
  -p, --port <PORT>          Port to run service on
  -u, --url <URL>            URL of the database
  -a, --aa-id <AA_ID>        Authorization Authority ID
  -B, --aa-addr <AA_ADDR>    Address of a Authorization Node [default: /ip4/127.0.0.1/tcp/5005]
  -i, --identity <IDENTITY>  Identity of Archive
  -h, --help                 Print help
  -V, --version              Print version

```

