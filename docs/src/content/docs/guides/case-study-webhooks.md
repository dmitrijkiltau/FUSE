---
title: "Case study: Webhook router"
description: A real-world example for validating and routing webhook payloads.
---

This example shows a small service that receives a webhook, validates its
payload, and routes it to different handlers based on event type.

## Data shapes

```
type Webhook:
  id: Id
  event: String(1..64)
  payload: Map<String, String>

enum Action:
  Stored
  Ignored(reason: String)
```

## Handlers

```
fn handle_created(hook: Webhook) -> Action:
  return Stored

fn handle_deleted(hook: Webhook) -> Action:
  return Ignored(reason="deletes ignored")
```

## Service

```
service Webhooks at "/api":
  post "/webhook" body Webhook -> Action!ValidationError:
    match hook.event:
      case "user.created":
        return handle_created(hook)
      case "user.deleted":
        return handle_deleted(hook)
      case _:
        return Ignored(reason="unknown event")
```
