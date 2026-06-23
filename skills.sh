#!/bin/sh
# Install the Mira agent skill into a Claude Code skills directory.
#
# The skill's one home is skills/mira/ in this repo; this script copies it into a
# Claude Code skills root so an agent can author and run Mira evals. It works two
# ways with no embedding and no extra dependency:
#
#   - From a checkout (this script sits next to skills/mira/): copies locally.
#   - Without a checkout: fetches each file from GitHub raw at --ref (default
#     main), so `curl … | sh` installs the skill on a box that only has the
#     prebuilt `mira` binary.
#
# Each run is a clean replace (the skill dir is removed first), so it doubles as
# the upgrade path and never leaves a file from an older version behind.
#
# Usage:
#   ./skills.sh [--global | --local] [--ref REF]
#   curl -fsSL https://raw.githubusercontent.com/everruns/mira/main/skills.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/everruns/mira/main/skills.sh | sh -s -- --global
#
#   --global   install into ~/.claude/skills (every project for this user)
#   --local    install into ./.claude/skills (this project; the default)
#   --ref REF  branch/tag/sha to fetch from when there's no local checkout
set -eu

REPO="everruns/mira"
SKILL="mira"
# The files that make up the skill. Keep in sync with skills/mira/.
FILES="SKILL.md references/cookbook.md references/scorers.md"

scope="local"
ref="main"
while [ $# -gt 0 ]; do
	case "$1" in
	--global) scope="global" ;;
	--local) scope="local" ;;
	--ref)
		ref="${2:?--ref needs a value}"
		shift
		;;
	--ref=*) ref="${1#*=}" ;;
	-h | --help)
		sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "skills.sh: unknown argument: $1" >&2
		exit 2
		;;
	esac
	shift
done

case "$scope" in
global) root="${HOME:?HOME is not set}/.claude/skills" ;;
local) root="$(pwd)/.claude/skills" ;;
esac
dest="$root/$SKILL"

# Prefer a local checkout (this script's own directory) so a dev install needs no
# network and pins to the working tree.
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -f "$script_dir/skills/$SKILL/SKILL.md" ]; then
	src="$script_dir/skills/$SKILL"
else
	src=""
fi

# Download a single file from GitHub raw, via curl or wget (whichever exists).
fetch() {
	url="https://raw.githubusercontent.com/$REPO/$ref/skills/$SKILL/$1"
	if command -v curl >/dev/null 2>&1; then
		curl -fsSL "$url" -o "$2"
	elif command -v wget >/dev/null 2>&1; then
		wget -qO "$2" "$url"
	else
		echo "skills.sh: need curl or wget to fetch the skill" >&2
		exit 1
	fi
}

# Clean replace: drop any prior install so a removed file can't linger.
rm -rf "$dest"
for f in $FILES; do
	out="$dest/$f"
	mkdir -p "$(dirname "$out")"
	if [ -n "$src" ]; then
		cp "$src/$f" "$out"
	else
		fetch "$f" "$out"
	fi
done

echo "installed the $SKILL skill to $dest"
