# nightly-backup

`backup.sh` snapshots a directory into timestamped folders and rotates old
ones. See the header of the script for the exact behaviour.

```sh
./backup.sh -n 7 /srv/uploads /mnt/backups/uploads
```

`tests/smoke.sh` runs a quick end-to-end check.
