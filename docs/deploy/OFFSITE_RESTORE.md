# Off-host backups and the restore-from-remote drill

## What runs
`kizuna-backup.timer` (daily 02:30, `Persistent=true`) → `kizuna-backup.service` (root, oneshot) →
`/opt/kizuna/scripts/offsite-backup.sh --db kizuna_pilot --storage /var/lib/kizuna/objects --out /var/backups/kizuna`:
1. `backup.sh`: pg_dump + object store tar, SHA-256 manifest, GnuPG AES-256 symmetric encryption, `.sha256` file.
2. `rclone copyto` of the `.tar.gpg` and `.sha256` to `$KIZUNA_RCLONE_REMOTE` (e.g. `gdrive:kizuna-pilot`), then
   `rclone check --one-way` (size + MD5 on Google Drive).
3. Retention only after a verified upload: remote files older than 60 days, local files older than 14 days
   (the backup just taken is never deleted).
4. Any failure → non-zero exit → `OnFailure=kizuna-alert@kizuna-backup.service` → e-mail/Telegram alert.

Only ciphertext leaves the host. The passphrase (`/etc/kizuna/backup.pass`) and `KIZUNA_DATA_KEY`
(`/etc/kizuna/kizuna.env`) are NOT in the backup and must be escrowed offline (password manager / sealed envelope).
Without the passphrase the off-host copies are useless; without the data key TOTP/WebAuthn enrolments are lost.

## Configure the remote (founder, once)
```sh
sudo install -d -m 700 /etc/kizuna/rclone
sudo RCLONE_CONFIG=/etc/kizuna/rclone/rclone.conf rclone config      # n → name "gdrive" → drive → scope "drive.file"
# headless server: choose "n" for auto config and run `rclone authorize "drive"` on a machine with a browser
echo 'KIZUNA_RCLONE_REMOTE=gdrive:kizuna-pilot' | sudo tee -a /etc/kizuna/backup.env
sudo systemctl start kizuna-backup.service && journalctl -u kizuna-backup -n 20
```
Use a dedicated Google account or a shared drive the company controls; scope `drive.file` limits rclone to files it
created. Until `KIZUNA_RCLONE_REMOTE` is set, backups stay local and the log says so on every run.

## Restore-from-remote drill (monthly, and after every upgrade)
Run on the pilot host (or any host with PostgreSQL, the KIZUNA roles and the same rclone config):
```sh
sudo -i
export RCLONE_CONFIG=/etc/kizuna/rclone/rclone.conf KIZUNA_BACKUP_PASSPHRASE_FILE=/etc/kizuna/backup.pass
F=$(/opt/kizuna/scripts/offsite-fetch.sh --remote gdrive:kizuna-pilot --db kizuna_pilot --to /var/backups/kizuna/drill-fetch)
cd /opt/kizuna && KIZUNA_RESTORE_VERIFY_BIN=/opt/kizuna/bin/kizuna-restore-verify KIZUNA_ENV_FILE=/etc/kizuna/drill.env \
  scripts/restore-drill.sh "$F" --tamper-test --report /var/backups/kizuna/drills
rm -rf /var/backups/kizuna/drill-fetch
```
`offsite-fetch.sh` downloads the newest backup (or `--name <file>`) and refuses it if its SHA-256 does not match.
`restore-drill.sh` decrypts, checks the manifest, restores into a throwaway `kizuna_drill_*` database (dropped
afterwards), verifies every tenant's audit hash chain and every artifact hash, and with `--tamper-test` proves that a
damaged object is detected. Pass = exit 0 and `"tamper_test": "detected"`. Keep the JSON reports.

## Real restore after losing the host
New host → install PostgreSQL + `deploy/install-pilot.sh` (creates empty `kizuna_pilot`) → stop `kizuna` →
`offsite-fetch.sh` → decrypt (`gpg -d … | tar -x`) → `pg_restore --clean -d kizuna_pilot db.dump` as postgres →
extract `objects.tar` into `/var/lib/kizuna/objects` (owner kizuna) → `kizuna-restore-verify` against both →
restore `kizuna.env` secrets from escrow → start.

## Evidence (dev host, 2026-11-05)
`scripts/tests/offsite-backup.test.sh` (real rclone 1.60 local remote + stub): upload, verify, remote/local retention,
fetch, tamper refusal, unreachable remote (non-zero, no retention), local-only warning — all pass.
Real chain on the dev host: `offsite-backup.sh` of a 38-tenant test database → local rclone remote →
`offsite-fetch.sh` → `restore-drill.sh --tamper-test`: exit 0, audit chains 38/38 intact, artifacts 48/48 intact,
tamper detected, restore 0.82 s, verify 0.24 s. Google Drive itself has not been exercised (needs the founder's
account).
