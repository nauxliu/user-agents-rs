# Upstream maintenance

Upstream: <https://github.com/intoli/user-agents>, branch `main`.

`upstream.lock.json` records the reviewed commit and a SHA-256/file-mode manifest. The initial port was reviewed against `f9b4344a0ea5427ccba51c0f7519174e837c4ab3`. Every tracked upstream file is monitored except these exact paths:

- `src/user-agents.json.gz` (validated separately as data)
- `README.md`, `CONTRIBUTING.md`, `CLA.md`
- `.github/pull_request_template.md`
- `media/facebook.png`, `media/reddit.png`, `media/twitter.png`, `media/ycombinator.png`

`package.json` is compared as canonical JSON after removing only its top-level `version` field. All dependency and script changes still fail. New paths are monitored by default, even outside `src/`. File modes are included, so replacing a monitored file with a symlink or changing its executable bit fails. Source is read from Git objects, avoiding checkout transformations. Neither the sync script nor CI installs upstream dependencies or executes upstream code.

The check compares the current upstream tree to the reviewed tree. A change that was completely reverted before the next sync leaves no tree difference. The workflow deliberately stops before syncing data if the source check fails and does not advance the reviewed baseline.

## When the Action fails on source changes

1. Read the changed paths in the failure log and compare upstream against `reviewed_commit` in `upstream.lock.json`.
2. Review and port relevant behavior into Rust; update tests as needed. Inspect dependency/build/license changes too.
3. In a local upstream clone at the reviewed new commit, explicitly regenerate the baseline:

   ```sh
   git clone https://github.com/intoli/user-agents.git /tmp/user-agents-review
   # Review /tmp/user-agents-review before running the next command.
   python3 scripts/sync_upstream.py review --checkout /tmp/user-agents-review
   python3 scripts/sync_upstream.py sync --checkout /tmp/user-agents-review
   ```

4. Run the development checks in the README. Commit the Rust changes, reviewed baseline, and data together, then rerun the Action.

The scheduled workflow never calls `review`. Do not regenerate the baseline just to silence a failure without reviewing the changes.

## Provenance and license

`data/upstream.json` records the exact source commit, SHA-256 of the gzip bytes, and profile count. The original gzip bytes are retained. New data is written only when the gzip changes; an unchanged download does not churn provenance commits. Python validates the gzip/JSON/schema/weights, then Rust tests parse and exercise the bundled dataset before the bot commits.

The library's selection/filtering behavior and dataset are derived from Intoli's BSD-2-Clause project. The original copyright notice and disclaimer are included in `LICENSE` and packaged with the crate. This is an independent Rust port, not an official Intoli release.
