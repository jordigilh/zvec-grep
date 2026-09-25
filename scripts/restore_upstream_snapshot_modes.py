"""Undo chmod-only churn from scip-go's snapshot updater in an isolated clone.

python3 scripts/restore_upstream_snapshot_modes.py /absolute/path/to/temp/scip-go
Touches only tracked internal/testdata/snapshots/output files that Git records
as executable in HEAD and whose current mode is non-executable. Never changes
content or files outside the temporary upstream checkout.
"""
import os
import pathlib
import subprocess
import sys


def run(root):
    root = pathlib.Path(root).resolve(strict=True)
    if root.name != 'scip-go-enclosing-spike-20260925':
        raise ValueError('only the dedicated isolated upstream clone is allowed')
    listing = subprocess.check_output(['git', 'ls-files', '-s', '-z', '--', 'internal/testdata/snapshots/output'], cwd=root)
    restored = 0
    for record in listing.split(b'\0'):
        if not record:
            continue
        metadata, path = record.split(b'\t', 1)
        mode = metadata.split(b' ', 1)[0]
        if mode != b'100755':
            continue
        target = root / path.decode('utf-8')
        if target.exists() and not target.stat().st_mode & 0o111:
            os.chmod(target, target.stat().st_mode | 0o111)
            restored += 1
    print(f'Restored original executable bit on {restored} generated snapshot files')


if __name__ == '__main__':
    run(sys.argv[1])
