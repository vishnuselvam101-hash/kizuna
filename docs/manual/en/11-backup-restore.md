# Backups and restore (operator)

This page is for the person who runs the KIZUNA server. Full commands are in
[docs/deploy/DEPLOYMENT.md](../../deploy/DEPLOYMENT.md) §7–8 and
[docs/deploy/OFFSITE_RESTORE.md](../../deploy/OFFSITE_RESTORE.md).

## What is backed up
`scripts/backup.sh` writes the database (`pg_dump`) and the document store into one archive, adds a SHA-256
manifest and encrypts it with GnuPG (AES-256). `kizuna-backup.timer` runs it daily at 02:30.
`scripts/offsite-backup.sh` also copies the encrypted file off the host with rclone, checks the copy, and only then
deletes old files (remote after 60 days, local after 14). Any failure sends an alert.

**Not in the backup:** the backup passphrase and `KIZUNA_DATA_KEY`. Keep both offline (password manager or sealed
envelope). Without the passphrase the backups are useless; without the data key all authenticator and security-key
enrolments are lost.

## Restore drill (monthly and after every upgrade)
`scripts/restore-drill.sh <backup> --tamper-test --report <dir>` decrypts the backup, restores it into a throwaway
database, verifies every organisation's audit hash chain and every document hash, and proves that a damaged file is
detected. Pass = exit code 0 and `"tamper_test": "detected"`. Keep the JSON reports.
For an off-host copy, fetch it first with `scripts/offsite-fetch.sh`, which refuses a file with the wrong SHA-256.

## Real restore
1. Stop the `kizuna` service.
2. Fetch and decrypt the backup.
3. Restore the database with `pg_restore` and extract the document store.
4. Run `kizuna-restore-verify` against both.
5. Restore the secrets (`kizuna.env`) from escrow, then start the service.
6. Check `/healthz` and run `kizuna-admin verify-audit <organisation>`.

## Upgrades
Back up and run the drill first. Migrations are forward-only: rollback means the previous binaries plus the
pre-upgrade backup.
