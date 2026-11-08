---
name: mock-demo
description: Use when asked to run the mock demo or demonstrate a simple skill and tool.
---

# Mock demo

1. Take the user's message, or use `hello` if none was supplied.
2. Call `mock_echo` with `{"message":"<message>"}`.
3. Return the tool's result and explain that this is placeholder behavior.

If the tool is unavailable or denied, report that instead of inventing a result.
