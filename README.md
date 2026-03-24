# Android Updater

Syncs local directories to an Android phone over ADB. Only files that are newer locally get pushed; extra files on the phone get deleted.

## Setup

1. Install `adb`
2. Copy `config.example` to `config.txt` and edit the mappings:
   ```
   /home/user/music -> /sdcard/Music
   ```
3. Connect your phone with USB debugging enabled
4. Run `cargo run` (or `cargo run -- --dry-run` to preview)

## Known Issues

> **DST Warning:** Files that differ by less than 1 hour are skipped to avoid
> false positives caused by daylight saving time shifts. If your local clock
> jumps forward or backward by an hour (e.g. spring/fall DST), the program
> will print a warning and skip those files rather than re-pushing them
> unnecessarily. Genuinely modified files will still be pushed as long as the
> time difference exceeds 1 hour.

> **ADB Push Retry:** After each `adb push`, the remote file size is verified.
> If the file is 0 bytes (a known ADB quirk), the push is retried up to 10
> times before the program exits with an error.
