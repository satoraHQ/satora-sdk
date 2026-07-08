---
"@lendasat/lendaswap-sdk-pure": minor
---

Add `getBulkStatus(ids)` to fetch the status of many swaps in a single request.

Returns `{ statuses, not_found }` — only each swap's status, so the whole batch is served by one database query. Unknown IDs are returned in `not_found` instead of throwing, so one bad ID does not fail the whole call. Backed by the new `POST /swap/bulk-status` endpoint (max 100 IDs per request).
