#!/usr/bin/env bash
# Fails when a tracked text file contains the em dash character (the
# project writes a plain "-" instead).
# THIRD-PARTY-NOTICES.md is generated from other projects' licence texts and is skipped.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
dash=$'\xe2\x80\x94'
if git grep -nI -F -e "$dash" -- . ':!THIRD-PARTY-NOTICES.md'; then
  echo "error: em dash found in the files above; use a plain dash" >&2
  exit 1
fi
echo "no em dash in tracked text files"
