---
"@satora/swap": minor
---

Let the Bitcoin confirmation depth change after the client is built.

`setBitcoinMinConfirmations` updates the depth a server-funded Bitcoin HTLC
needs before it reads as claimable, so callers that cache a single client can
change it without rebuilding and orphaning tracking already in flight. It
applies to every swap the client tracks, including ones already in flight.

Only the chain monitors read a depth, so it needs `withChainVerifiedTracking`;
setting one without it now warns rather than silently doing nothing. A depth
that is not a whole block count is rejected instead of stalling every read.
