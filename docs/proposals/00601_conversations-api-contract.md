---
issue: https://github.com/praxis-proxy/praxis/issues/601
discussion: # add PR link after submission
status: proposed
authors:
  - leseb
graduation_criteria:
  - Contract accepted by stakeholders
  - Response shapes adopted by handler implementation issues
  - Storage integration reviewed against #596 epic scope
stakeholders:
  - shaneutt
  - nerdalert
  - twghu
---

# Conversations API: Proxy Contract and Validation Rules

## What?

Define the proxy-owned OpenAI-compatible `/v1/conversations`
contract that Praxis will implement for vLLM-backed
deployments.

This contract pins the endpoint surface, request and response
object shapes, accepted `conversation` forms in
`POST /v1/responses`, ID format expectations, error
semantics, and interaction rules between conversations and
the existing Responses persistence layer — before handler
and store work spreads those decisions across the
implementation.

### Goals

- Pin the `/v1/conversations` endpoint surface and HTTP
  semantics so implementation issues share a single source
  of truth.
- Define the two accepted `conversation` forms in
  `POST /v1/responses`: bare string ID and object with `id`.
- Define precedence when `previous_response_id` and
  `conversation` are both present.
- Specify error shapes and status codes for missing, deleted,
  malformed, and cross-tenant conversation IDs.
- Decide which item types are persisted verbatim versus
  normalized.
- Specify how `store`, streaming, and background options
  interact with conversation append behavior.
- Capture known OpenAI compatibility gaps as explicit
  non-goals.

## Why?

### Motivation

The Conversations API touches routing, persistence,
rehydration, and public compatibility behavior. Without a
small contract step, handler and store implementations will
make independent decisions about object shapes, error
formats, and edge-case behavior — leading to inconsistency
and churn.

vLLM does not implement `/v1/conversations`. Praxis must own
this control plane locally, using the existing
`ResponseStore` path as the single storage source of truth.
A written contract ensures every implementation issue works
against the same shapes.

### User Stories

- As an AI application developer, I want to create and
  manage conversation objects via `/v1/conversations` so
  that I can persist multi-turn context without manually
  chaining `previous_response_id`.
- As a platform engineer, I want the proxy to handle
  `/v1/conversations` locally so that vLLM-backed
  deployments get conversation support without upstream
  changes.
- As a multi-tenant SRE, I want conversation operations
  scoped to tenants so that cross-tenant access returns
  the same 404 as "not found" — no information leakage.

## How?

### Routing Principle

All `/v1/conversations` endpoints are handled locally by
Praxis. They are **never** forwarded to the upstream
inference server (vLLM). The existing `ResponseStore` trait
and its SQLite / PostgreSQL backends are the single source
of truth for conversation state.

`POST /v1/responses` continues to be forwarded to vLLM for
inference. The conversation integration happens in Praxis
filters before and after the upstream call: the rehydrate
filter prepends conversation history into the request, and
the store filter appends input/output items back into the
conversation after the response completes.

### Storage Source of Truth

Conversation data lives in the existing `conversation_messages`
table managed by the `ResponseStore` trait
(`filter/src/builtins/http/ai/store/trait_def.rs`). The
current schema is:

```sql
CREATE TABLE IF NOT EXISTS {conversations_table} (
    conversation_id TEXT NOT NULL,
    tenant_id       TEXT NOT NULL,
    messages        TEXT NOT NULL,  -- JSON
    PRIMARY KEY (conversation_id, tenant_id)
);
```

Implementation issues will extend this schema to support the
full conversation object (metadata, created_at) and
individual item storage. The `ResponseStore` trait will gain
the necessary CRUD methods. No separate storage subsystem is
introduced.

---

### Endpoint Contract

#### 1. Create a Conversation

```
POST /v1/conversations
```

**Request body (JSON):**

| Field      | Type            | Required | Description                                         |
|------------|-----------------|----------|-----------------------------------------------------|
| `items`    | array of Item   | No       | Initial context items, max 20 per call.             |
| `metadata` | object          | No       | Up to 16 key-value pairs (keys <= 64, values <= 512 chars). |

**Response (201):**

```json
{
  "id": "conv_abc123",
  "object": "conversation",
  "created_at": 1741900000,
  "metadata": {}
}
```

**Notes:**
- Praxis generates the `conv_` prefixed ID using the same
  hex ID generator as response IDs.
- Items provided in the create request are persisted
  immediately and are retrievable via the items endpoints.
- The response does NOT echo back the items array; clients
  use `GET .../items` to retrieve them.

---

#### 2. Retrieve a Conversation

```
GET /v1/conversations/{conversation_id}
```

**Response (200):**

```json
{
  "id": "conv_abc123",
  "object": "conversation",
  "created_at": 1741900000,
  "metadata": {"topic": "project-x"}
}
```

---

#### 3. Update a Conversation

```
POST /v1/conversations/{conversation_id}
```

**Request body (JSON):**

| Field      | Type   | Required | Description                              |
|------------|--------|----------|------------------------------------------|
| `metadata` | object | No       | Replaces the full metadata map.          |

**Response (200):** Updated conversation object (same shape
as retrieve).

**Notes:**
- Only `metadata` is mutable. The `id` and `created_at`
  fields are immutable.
- Sending `metadata: {}` clears all metadata.
- Sending `metadata: null` or omitting `metadata` is a
  no-op; the existing metadata is preserved.

---

#### 4. Delete a Conversation

```
DELETE /v1/conversations/{conversation_id}
```

**Response (200):**

```json
{
  "id": "conv_abc123",
  "object": "conversation.deleted",
  "deleted": true
}
```

**Notes:**
- Deleting a conversation deletes all its items.
- Responses that reference this conversation via
  `conversation` are NOT deleted; they remain retrievable
  independently via `GET /v1/responses/{id}`.

---

#### 5. Create Items

```
POST /v1/conversations/{conversation_id}/items
```

**Request body (JSON):**

| Field   | Type          | Required | Description                     |
|---------|---------------|----------|---------------------------------|
| `items` | array of Item | Yes      | Items to append, max 20.       |

**Response (200):** The parent conversation object (not the
items themselves). This matches OpenAI's behavior where the
response is the conversation object, not an item list.

---

#### 6. List Items

```
GET /v1/conversations/{conversation_id}/items
```

**Query parameters:**

| Param   | Type   | Default | Description                       |
|---------|--------|---------|-----------------------------------|
| `limit` | number | 20      | 1–100.                            |
| `after` | string | —       | Item ID cursor for pagination.    |
| `order` | string | `desc`  | `asc` or `desc` by creation order.|

**Response (200):**

```json
{
  "object": "list",
  "data": [ /* Item objects */ ],
  "first_id": "item_abc",
  "last_id": "item_xyz",
  "has_more": false
}
```

**Notes:**
- Forward-only cursor pagination via `after`.
- No `before` parameter (matches OpenAI).

---

#### 7. Retrieve an Item

```
GET /v1/conversations/{conversation_id}/items/{item_id}
```

**Response (200):** The item object.

---

#### 8. Delete an Item

```
DELETE /v1/conversations/{conversation_id}/items/{item_id}
```

**Response (200):** The parent conversation object.

---

### Conversation Parameter in `POST /v1/responses`

The `conversation` field in a Responses API request accepts
two forms:

#### Form 1: String ID

```json
{
  "model": "gpt-4.1",
  "input": "Hello",
  "conversation": "conv_abc123"
}
```

#### Form 2: Object with `id`

```json
{
  "model": "gpt-4.1",
  "input": "Hello",
  "conversation": {"id": "conv_abc123"}
}
```

Both forms are equivalent. The validate filter extracts the
conversation ID from either form and promotes it to
`responses.conversation_id` metadata.

**Current state:** The validate filter already handles
Form 2 (`conversation.id` extraction). Form 1 (bare string)
requires an update to `extract_conversation_id()` in
`filter/src/builtins/http/ai/openai/responses/validate/mod.rs`.

#### Auto-creation

When `conversation` is absent from a `POST /v1/responses`
request, the validate filter already generates a
`conv_`-prefixed ID. Whether this auto-generated
conversation is actually persisted depends on the `store`
flag:

- `store: true` (default) — the conversation is created in
  the store after the response completes.
- `store: false` — no conversation is persisted.

When `conversation` references an ID that does not exist:
the request fails with a 404 error (see Error Semantics
below). Praxis does not auto-create conversations from bare
ID references — the client must create the conversation
first via `POST /v1/conversations` or omit the field to
get auto-creation.

---

### Precedence: `previous_response_id` vs `conversation`

When both `previous_response_id` and `conversation` are
present in the same request:

1. **`previous_response_id` takes precedence** for history
   rehydration. The rehydrate filter loads context from the
   previous response chain, not from the conversation items.

2. The conversation ID is still recorded. After the response
   completes, input and output items are appended to the
   specified conversation (if `store: true`).

3. This means a request can rehydrate from a response chain
   while simultaneously building a conversation's item
   history — the two state mechanisms are not mutually
   exclusive for writes, but `previous_response_id` wins
   for reads.

**Rationale:** This matches the epic's requirement
("Preserve `previous_response_id` precedence when both
`previous_response_id` and `conversation` are present")
and avoids ambiguity about which history the model sees.

---

### Conversation Append Behavior

After a successful `POST /v1/responses`, items are appended
to the conversation when ALL of these conditions are met:

| Condition                    | Requirement                  |
|------------------------------|------------------------------|
| `store`                     | `true` (the default)          |
| `conversation`              | Present (explicit or auto-generated) |
| Response status              | `completed`                  |

Items are **not** appended when:

- `store: false` — no persistence of any kind.
- The response status is `failed`, `cancelled`, or
  `incomplete` — partial results are not appended to avoid
  corrupting conversation state.
- `background: true` — items are appended only after the
  background job completes successfully, not at request
  time.
- `stream: true` — items are appended after the stream
  completes, once the full response is materialized.

#### What gets appended

For each completed response:

1. **Input items** from the request's `input` field are
   appended first, in order.
2. **Output items** from the response (messages, tool calls,
   tool outputs) are appended after input items, in order.

Items are persisted **verbatim** as JSON — Praxis does not
normalize, transform, or validate item internals beyond
what it needs for its own operation (rehydration context
building). Unknown item types and unknown fields within
items are preserved as-is.

---

### Item Persistence: Verbatim vs Normalized

**Verbatim (default):** All item types are stored as
received from the client request or upstream response.
Praxis treats item JSON as opaque blobs. This includes:

- `message` items (all roles)
- `function_call` and `function_call_output` items
- `file_search_call`, `web_search_call` items
- `computer_call`, `computer_call_output` items
- `mcp_call`, `mcp_call_output` items
- `reasoning` items
- Any future item types OpenAI introduces

**Normalized (Praxis-internal):** The hidden `messages`
column in `ResponseRecord` contains a proxy-internal
representation used for rehydration. This is NOT the
conversation items — it is a pre-built Chat Completions
`messages` array that the rehydrate filter can inject
directly into upstream requests. This normalization is
an internal implementation detail of the rehydration
pipeline.

**Principle:** Conversation items returned via
`GET /v1/conversations/{id}/items` are always the verbatim
stored JSON, never the normalized internal messages.

---

### Error Semantics

All error responses use the standard OpenAI error envelope:

```json
{
  "error": {
    "message": "Human-readable description",
    "type": "error_type",
    "code": "error_code"
  }
}
```

#### Conversation Endpoint Errors

| Scenario                          | Status | `type`                    | `code`          |
|-----------------------------------|--------|---------------------------|-----------------|
| Conversation not found            | 404    | `not_found`               | `not_found`     |
| Cross-tenant access               | 404    | `not_found`               | `not_found`     |
| Malformed conversation ID         | 400    | `invalid_request_error`   | `invalid_id`    |
| Invalid metadata (>16 keys)       | 400    | `invalid_request_error`   | `invalid_value` |
| Items array exceeds 20            | 400    | `invalid_request_error`   | `invalid_value` |
| Item not found                    | 404    | `not_found`               | `not_found`     |
| Missing required field            | 400    | `invalid_request_error`   | `missing_field` |
| Invalid JSON body                 | 400    | `invalid_request_error`   | `invalid_json`  |
| Delete on already-deleted conv    | 404    | `not_found`               | `not_found`     |

**Cross-tenant behavior:** A conversation belonging to
tenant A is invisible to tenant B. All cross-tenant
lookups return 404 with the same shape as "not found" — no
information leakage about whether the ID exists for another
tenant. This matches the existing `ResponseStore` pattern
where `get_response` returns `None` for both "not found"
and "wrong tenant".

#### Responses Endpoint Conversation Errors

| Scenario                                         | Status | `type`                  | `code`          |
|--------------------------------------------------|--------|-------------------------|-----------------|
| `conversation` references non-existent ID        | 404    | `not_found`             | `not_found`     |
| `conversation` references cross-tenant ID        | 404    | `not_found`             | `not_found`     |
| `conversation` has malformed value (not string or `{id}`) | 400 | `invalid_request_error` | `invalid_value` |
| `conversation.id` is null or empty               | 400    | `invalid_request_error` | `invalid_value` |

---

### ID Format

- **Conversation IDs:** `conv_` prefix + 32 lowercase hex
  characters (same generator as `resp_` IDs).
  Example: `conv_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6`.
- **Item IDs:** `item_` prefix + 32 lowercase hex
  characters. Generated by Praxis when items are created
  (either via the items endpoint or via response append).
- IDs are immutable once assigned. Clients cannot choose
  or override IDs.

---

### Tenant Isolation

Tenant scoping follows the existing pattern:

1. An upstream authentication or multi-tenancy filter sets
   `responses.tenant_id` in filter metadata.
2. All store operations (conversation CRUD, item CRUD)
   include `tenant_id` in their queries.
3. Single-tenant deployments use the sentinel value
   `"default"` (constant `DEFAULT_TENANT_ID` in the store
   filter).
4. The conversation table's primary key is
   `(conversation_id, tenant_id)`, preventing cross-tenant
   access at the storage level.

---

### Non-Goals and Known Compatibility Gaps

These are explicitly out of scope for the first pass:

| Gap                              | Rationale                                        |
|----------------------------------|--------------------------------------------------|
| `GET /v1/conversations` (list)   | OpenAI does not expose a list conversations endpoint. Not needed for first pass. |
| Conversation-level TTL override  | OpenAI conversations bypass the 30-day response TTL. Praxis has no TTL mechanism yet; adding one is a separate concern. |
| `include` query parameter        | The list items `include` param (e.g., `file_search_call.results`, `reasoning.encrypted_content`) requires per-type enrichment logic. Deferred. |
| Full item type validation        | Praxis stores items as opaque JSON. Validating the ~30 item type variants is not needed for persistence. |
| Checkpointing / resume           | Conversation checkpointing is not part of the OpenAI Conversations API and is out of scope. |
| Permissions / sharing            | No per-conversation permission model beyond tenant isolation. |
| Conversation search / filtering  | No query-by-metadata or full-text search over conversations. |
| Streaming conversation events    | No dedicated SSE events for conversation mutations (e.g., `conversation.item.created`). |
| vLLM changes                     | All conversation handling is Praxis-local. No upstream protocol changes. |

---

### Implementation Issues

The following issues should reference this contract for
their object shapes and error formats:

- **Store trait extension:** Add `list_items`,
  `get_item`, `create_items`, `delete_item`,
  `create_conversation`, `update_conversation` methods
  to `ResponseStore`. Extend the `ConversationRecord`
  type to include `created_at` and `metadata`.
- **Schema migration:** Add `created_at` (BIGINT) and
  `metadata` (TEXT/JSON) columns to the conversations
  table. Add an `items` table with `(item_id, tenant_id,
  conversation_id)` primary key and `item_data`
  (TEXT/JSON), `created_at` (BIGINT), `position`
  (INTEGER) columns.
- **Conversation filter:** New `openai_conversations`
  filter that intercepts `/v1/conversations` requests
  and handles all CRUD operations locally.
- **Validate filter update:** Update
  `extract_conversation_id()` to accept both string and
  object forms. Add existence check for referenced
  conversation IDs.
- **Rehydrate filter:** Load conversation items when
  `conversation` is present and `previous_response_id`
  is absent.
- **Store filter update:** Append input/output items to
  the conversation after successful response persistence.
