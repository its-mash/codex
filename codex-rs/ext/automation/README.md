# codex-automation (fork)

Fork-owned extension providing durable, per-thread automation for native Codex
teammates: `loop_*`/`cron_*`/`monitor_*` tools plus Claude-compatible aliases
(`CronCreate`, `CronList`, `CronDelete`, `Monitor`), and the app-server
`loop/create`, `loop/list`, `loop/delete` requests (implemented in
`app-server/src/request_processors/thread_processor/loop_automation.rs`).

## App-server API

### Manage durable loops

`loop/create`, `loop/list`, and `loop/delete` control the same durable scheduler exposed to a thread's native automation tools. A fired loop enters through the thread's inter-agent mailbox and wakes an idle thread; clients do not need to start a model turn when creating or deleting it. Timestamps are Unix seconds. `loop/list` accepts optional `cursor` and `limit` fields and returns `nextCursor`.

```json
{ "method": "loop/create", "id": 26, "params": {
  "threadId": "thr_b",
  "prompt": "Reconcile the mock task board.",
  "everySeconds": 900,
  "name": "reconcile"
} }
{ "id": 26, "result": { "loop": {
  "id": "0198...", "name": "reconcile", "prompt": "Reconcile the mock task board.",
  "everySeconds": 900, "enabled": true, "createdAt": 1786450000,
  "nextDueAt": 1786450900, "lastRunAt": null
} } }

{ "method": "loop/list", "id": 27, "params": {
  "threadId": "thr_b", "cursor": null, "limit": 100
} }
{ "id": 27, "result": { "data": [ ... ], "nextCursor": null } }

{ "method": "loop/delete", "id": 28, "params": {
  "threadId": "thr_b", "id": "0198..."
} }
```

