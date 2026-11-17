#!/usr/bin/env python3
"""Reject private artifacts and session links; optionally inspect all reachable history."""
import argparse
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent.parent
SESSION = re.compile(rb'https://(?:claude\.ai/(?:code/session_|chat/)|chatgpt\.com/(?:c|share)/)[A-Za-z0-9_-]{8,}|(?im:^Claude-Session\s*:)')
PRIVATE_PATH = re.compile(r'(^|/)(?:\.claude|\.codex|\.codex-attachments|themes\.local)(/|$)|(^|/)bundle-id\.local$|(^|/)\.env(?:$|\.)|\.(?:p12|pfx|mobileprovision|bundle)$|(^|/)AmazonEmber[^/]*\.ttf$|^resources/themes/.*\.json$')


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT)


def check(history=False):
    errors = []
    if history:
        names = git('log', '--all', '--format=', '--name-only', '-z').split(b'\0')
        paths = {p.strip(b'\n').decode(errors='replace') for p in names if p.strip(b'\n')}
        objects = git('rev-list', '--objects', '--all', '--no-object-names').splitlines()
    else:
        paths = {p.decode() for p in git('ls-files', '-z').split(b'\0') if p}
        objects = []
        for path in paths:
            file = ROOT / path
            if file.is_file() and SESSION.search(file.read_bytes()):
                errors.append(f'{path}: private session reference')
    for path in sorted(paths):
        if PRIVATE_PATH.search(path):
            errors.append(f'{path}: private or excluded distribution artifact')
    if history:
        process = subprocess.Popen(['git', 'cat-file', '--batch'], cwd=ROOT,
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        try:
            for oid in objects:
                process.stdin.write(oid + b'\n')
                process.stdin.flush()
                header = process.stdout.readline().split()
                data = process.stdout.read(int(header[2]))
                process.stdout.read(1)
                if SESSION.search(data):
                    errors.append(f'{oid.decode()}: private session reference')
        finally:
            process.stdin.close()
            process.stdout.close()
            process.wait()
        # Upstream contributor identities are legal attribution, not our private metadata.
        for email in set(git('log', 'upstream-baseline..', '--all', '--format=%ae%n%ce').splitlines()):
            if email and not email.endswith((b'@users.noreply.github.com', b'.invalid')):
                errors.append('Fork author/committer email must use a noreply or private-placeholder address')
                break
    if errors:
        raise SystemExit('\n'.join(errors))
    print(f'Publication hygiene: {len(paths)} paths' + (f', {len(objects)} reachable objects' if history else ''))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--history', action='store_true')
    check(parser.parse_args().history)
