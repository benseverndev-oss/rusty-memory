#!/usr/bin/env bash
#
# rmem as a Claude Code hook: remembering stops being the agent's choice.
#
# Three modes, wired to two hooks on UserPromptSubmit:
#
#   recall   synchronous. Prints one JSON object whose additionalContext is
#            what the store already holds near this prompt.
#   spool    appends the prompt to a queue and detaches a drainer. Instant, and
#            it cannot fail in a way that costs the turn.
#   drain    takes the queue apart one line at a time, calling `rmem remember`.
#            At most one runs at a time; a second exits rather than queues.
#
# # Why a queue rather than just calling `rmem remember`
#
# `rmem remember` holds the store's exclusive lock across its extraction call
# and its embeddings -- seconds, not milliseconds, because the model is on the
# other side of the network. The lock waits 5s and then refuses. So two prompts
# submitted close together, or one prompt arriving while the MCP server is
# mid-write, would lose a turn outright and say so only in a log.
#
# Spooling separates the part that must not fail (recording that the turn
# happened) from the part that is slow and contended (extracting it). The spool
# append is a flock and an echo. The drain is serialised against itself by a
# second lock taken non-blocking: a second drainer exits rather than queues,
# because the drainer already holding the lock will reach the line it would
# have processed.
#
# # Configuration
#
#   RMEM_HOOK_DIR       directory holding rmem.toml and the store.
#                       Default: $CLAUDE_PROJECT_DIR/.rmem
#   RMEM_BIN            path to the rmem binary. Default: rmem, from PATH.
#   RMEM_HOOK_SPEAKER   --speaker for remembered turns. Default: user
#   RMEM_HOOK_MIN_CHARS prompts shorter than this are not remembered.
#                       Default: 24. Every remembered turn costs a completion
#                       and an embedding, and "yes" is not worth either.
#   RMEM_HOOK_K         how many hits recall injects. Default: 5
#   RMEM_HOOK_OFF       set to 1 to make every mode a no-op.
#
# # This script never writes to stdout except in `recall`
#
# On UserPromptSubmit, anything a hook prints that is not the JSON envelope is
# injected into the model's context verbatim. A stray progress line from
# `remember` would become something the agent reads as though the user typed
# it. Every mode below sends its own output to the log instead.

set -uo pipefail

mode="${1:-}"

if [ "${RMEM_HOOK_OFF:-0}" = "1" ]; then
  exit 0
fi

dir="${RMEM_HOOK_DIR:-${CLAUDE_PROJECT_DIR:-$PWD}/.rmem}"
bin="${RMEM_BIN:-rmem}"
speaker="${RMEM_HOOK_SPEAKER:-user}"
min_chars="${RMEM_HOOK_MIN_CHARS:-24}"
k="${RMEM_HOOK_K:-5}"

spool="$dir/hook-spool.jsonl"
spool_lock="$dir/hook-spool.lock"
drain_lock="$dir/hook-drain.lock"
log="$dir/hook.log"

# A missing store is not an error to report at a prompt. It means rmem was
# never set up here, and the right behaviour is to stay out of the way.
[ -d "$dir" ] || exit 0
[ -f "$dir/rmem.toml" ] || exit 0
command -v jq >/dev/null 2>&1 || exit 0
command -v "$bin" >/dev/null 2>&1 || [ -x "$bin" ] || exit 0

note() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >>"$log" 2>/dev/null || true; }

case "$mode" in

  recall)
    prompt="$(jq -r '.prompt // empty' 2>/dev/null)" || exit 0
    [ -n "$prompt" ] || exit 0
    [ "${#prompt}" -ge "$min_chars" ] || exit 0

    hits="$(cd "$dir" && timeout 8 "$bin" recall "$prompt" -k "$k" 2>>"$log")" || exit 0
    # The empty-store wording is a sentence, not a list; injecting it would
    # spend context to say nothing.
    case "$hits" in ""|"nothing recalled"*) exit 0 ;; esac

    jq -n --arg hits "$hits" '{
      hookSpecificOutput: {
        hookEventName: "UserPromptSubmit",
        additionalContext:
          ("Your rmem store already holds these, nearest first. They are what
was recorded, not necessarily what is true now -- a bracketed note means a
later assertion exists. Use the rmem tools to read an entity in full or to
list decisions.\n\n" + $hits)
      }
    }'
    ;;

  spool)
    prompt="$(jq -r '.prompt // empty' 2>/dev/null)" || exit 0
    [ -n "$prompt" ] || exit 0
    [ "${#prompt}" -ge "$min_chars" ] || exit 0

    # One JSON object per line, so a prompt containing newlines stays one
    # entry. The lock is held for an append and nothing else.
    line="$(jq -cn --arg t "$prompt" '{text: $t}')" || exit 0
    ( flock 9; printf '%s\n' "$line" >>"$spool" ) 9>"$spool_lock" || note "spool failed"

    # Detached rather than left to the hook runner. `async: true` would also
    # keep this off the prompt's critical path, but then whether the turn is
    # ever recorded depends on a settings field being spelled right, and a
    # hook that silently runs synchronously would put a network round trip in
    # front of every prompt. setsid with all three streams closed is the same
    # guarantee from inside the script, where it can be tested. Redundant with
    # async, and harmless alongside it: the drain lock makes a second drainer
    # a no-op.
    setsid "$0" drain </dev/null >/dev/null 2>&1 &
    ;;

  drain)
    # Non-blocking: if a drainer holds this, it will reach whatever was just
    # spooled, and a second one waiting would only make them alternate.
    exec 9>"$drain_lock"
    flock -n 9 || exit 0

    while :; do
      # Pop under the spool lock so a concurrent `spool` never loses its
      # append to this rewrite.
      line="$(
        exec 8>"$spool_lock"
        flock 8
        [ -s "$spool" ] || exit 1
        head -n 1 "$spool"
        tail -n +2 "$spool" >"$spool.next" && mv "$spool.next" "$spool"
      )" || break
      [ -n "$line" ] || continue

      text="$(printf '%s' "$line" | jq -r '.text // empty')" || continue
      [ -n "$text" ] || continue

      if ! ( cd "$dir" && "$bin" remember "$text" --speaker "$speaker" ) >>"$log" 2>&1; then
        note "remember failed; the turn above was not recorded"
      fi
    done
    ;;

  *)
    echo "usage: rmem-hook.sh recall|spool|drain  (JSON on stdin for recall and spool)" >&2
    exit 2
    ;;
esac
