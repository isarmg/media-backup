# Photo Backup Server 0.2.0

This archive is the complete `x86_64-unknown-linux-gnu` server release. It contains the real server binary and embedded-Web inspection copies, the strict release manifest, the production environment template, the systemd unit, the public 0.2 mobile FFI header, and every operational script referenced below. Installation requires systemd, Python 3, and GNU coreutils/tar.

Before installation, verify the checksum published beside the archive, then verify the extracted immutable payload without supplying application configuration:

```bash
./bin/photo-backup-server release-identity
./bin/photo-backup-server release-verify "$PWD"
```

Install on Debian, Ubuntu, or WSL with systemd:

```bash
sudo ./scripts/setup-wsl.sh
sudoedit /etc/isarmg/photo-backup.env
sudo /opt/isarmg/photo-backup/current/scripts/start-server-wsl.sh
```

`setup-wsl.sh` installs the entire verified generation at `/opt/isarmg/photo-backup/releases/0.2.0`, atomically points `current` to it, creates separate SQLite and object roots below `/var/lib/isarmg/photo-backup`, and creates but never prints random initial secrets. It does not start the service. Replace both generated secrets and remove the `# INITIAL-SECRETS-MUST-BE-REPLACED` line before invoking the start script.

To start without enabling the service and then follow its journal, run:

```bash
sudo /opt/isarmg/photo-backup/current/scripts/run-server-wsl.sh
```

After configuring TLS termination, `scripts/verify-server-wsl.sh` checks the live health and admin endpoints. The server process initializes only a missing Photo Backup 0.2.0 SQLite database and otherwise requires the exact current Schema. Product code has no cross-version migrate, backup, or restore command; use the separately distributed upgrade tool while the service is stopped.

All first-party contents are licensed under the Apache License 2.0 in `LICENSE`.
