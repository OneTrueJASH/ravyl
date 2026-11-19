#!/bin/sh
# Mirror the engine (crates/ravyn) and its data files into a checkout of
# the standalone RAVYN repository, then show what changed there. Files
# that only the standalone repository has (README, CONTRIBUTING, SECURITY,
# CI, lockfile, .gitignore, banner) are left alone; everything else mirrors crates/ravyn,
# deletions included. Review and commit in the checkout.
#
#   scripts/sync-ravyn.sh [path to the ravyn checkout]   (default ../ravyn)
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
dest=${1:-"$here/../ravyn"}
if [ ! -d "$dest/.git" ]; then
    echo "sync-ravyn: $dest is not a git checkout of ravyn" >&2
    exit 1
fi
dest=$(cd "$dest" && pwd)
if [ ! -d "$dest/.git" ]; then
    echo "sync-ravyn: $dest is not a git checkout of ravyn" >&2
    exit 1
fi

rsync -a --delete \
    --exclude /.git/ --exclude /target/ --exclude /data/ \
    --exclude /README.md --exclude /.github/ --exclude /Cargo.lock \
    --exclude /.gitignore --exclude /fuzz/target/ --exclude /assets/ \
    --exclude /CONTRIBUTING.md --exclude /SECURITY.md \
    "$here/crates/ravyn/" "$dest/"
mkdir -p "$dest/data"
# Only the leave values: the word list comes from the data repository.
cp "$here/data/leaves.rlv" "$dest/data/"

rev=$(git -C "$here" rev-parse --short HEAD)
echo "Synced from ravyl $rev. Changes in $dest:"
git -C "$dest" status --short
echo "Commit there with: git -C $dest commit -am 'Sync from ravyl $rev'"
