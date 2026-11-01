#!/usr/bin/env python3
"""Copy the PostHog project token and region from .env into the release workflow's GitHub secrets."""
import argparse
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--env', type=Path, default=root / '.env')
parser.add_argument('--repo', help='owner/name; defaults to the checkout\'s GitHub repository')
args = parser.parse_args()

values = {}
for line in args.env.read_text().splitlines():
    if '=' in line and not line.lstrip().startswith('#'):
        key, value = line.split('=', 1)
        values[key.strip()] = value.strip().strip('"\'').strip()
token, region = values.get('POSTHOG_TOKEN', ''), values.get('POSTHOG_REGION', '')
assert token and region, f'{args.env} must set POSTHOG_TOKEN and POSTHOG_REGION'
# Only the write-only project token ships in builds; personal API keys grant account access.
assert token.startswith('phc_') and token.replace('_', '').isalnum(), 'POSTHOG_TOKEN must be a PostHog project token (phc_...)'

gh = ['gh', 'secret', 'set'] + (['--repo', args.repo] if args.repo else [])
for name, value in (('POSTHOG_TOKEN', token), ('POSTHOG_REGION', region)):
    # The value goes through stdin so it never appears in process listings.
    subprocess.run(gh + [name], input=value, text=True, check=True, cwd=root)
print(subprocess.check_output(['gh', 'secret', 'list'] + (['--repo', args.repo] if args.repo else []), text=True, cwd=root), end='')
