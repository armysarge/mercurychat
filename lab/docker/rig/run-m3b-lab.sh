#!/usr/bin/env bash
# The M3b rig lab exit, now one mode of run-rig-lab-checks.sh.
exec bash "$(dirname "$0")/run-rig-lab-checks.sh" m3b "$@"
