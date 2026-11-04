#!/bin/sh
# Report to tome that this agent is waiting for input (`blocked`) or working
# again (`unblocked`), from any harness's hooks. Does nothing outside a tome
# run. `--json` prints `{}` for harnesses that read a hook's reply (Cursor). A marker file keeps
# `unblocked`, which runs after every tool call, from reaching the daemon
# unless the agent was blocked.
[ "$2" = "--json" ] && echo '{}'
[ -n "$TOME_RUN_ID" ] || exit 0
command -v tome >/dev/null 2>&1 || exit 0
marker="${TMPDIR:-/tmp}/tome-blocked-$TOME_RUN_ID-${TOME_WORKER_ID:-agent}"
case "$1" in
  blocked) touch "$marker" ;;
  unblocked) [ -e "$marker" ] || exit 0; rm -f "$marker" ;;
  *) exit 0 ;;
esac
if [ -n "$TOME_WORKER_ID" ]; then
  tome worker "$1" >/dev/null 2>&1
else
  tome step "$1" >/dev/null 2>&1
fi
exit 0
