# Empty material workspace — authorized read-only diagnostic

User explicitly requested a quick database check before continuing priorities.
Independent diagnostic plan reviewer `ses_ec20c3d40ffeWgAu2iVKf5PFmW`: GO;
require one coherent read transaction and stop without changes if read-only/WAL
access fails. No implementation change was needed.

Executed metadata-only process inspection identified running Mimik PID377502,
working directory `/home/marco/Progetti/mimik`, installed binary
`target/release/mimik` (18,210,072bytes), and open database
`/home/marco/Progetti/mimik/data/mimik.sqlite3`. No process arguments or environment
were read. lsof warned about unrelated Docker network namespace visibility; the
Mimik database path was available.

Existing container, network disabled, read-only root and read-only selected data
directory mount: Python sqlite3 URI `mode=ro`, `query_only=ON`,
`trusted_schema=OFF`, explicit read transaction. Only schema version and aggregate
material table counts were queried; transaction rolled back/connection closed.

| Query | Result |
|---|---:|
| Schema version |15|
| sources |0|
| source_pages |0|
| source_units |0|
| source_review_events |0|

Disposition: empty material page is consistent with the running server's actual
database, rather than evidence of a listing bug. The historical MÖRK BORG sample
is absent from this file. This does not establish when/why it disappeared or that
the database is otherwise empty. No credential/account/session fields, manual or
extracted content were read; no migration, write, reset, recovery or import occurred.
User accepts testing material import later; continue registry/definition browser work.
