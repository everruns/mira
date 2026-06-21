#!/usr/bin/env sh
# A stand-in "agent under test": reads the prompt as argv[1] and upper-cases it.
# Replace this with any external binary in any language — Mira drives it the same
# way, capturing stdout as the transcript.
printf '%s' "$1" | tr 'a-z' 'A-Z'
