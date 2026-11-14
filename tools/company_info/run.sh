#!/bin/sh
# Example tool. Arguments arrive as JSON on stdin and as ARG_<NAME> env vars.
# Print the result to stdout; exit non-zero to report an error.
case "$ARG_TOPIC" in
  name)  echo "umango Inc." ;;
  team)  echo "Uri (founder)" ;;
  stack) echo "Rust workspace: crates/core (backends, tools) + crates/cli (terminal UI)" ;;
  *)     echo "unknown topic '$ARG_TOPIC' (use name, team or stack)" >&2; exit 1 ;;
esac
