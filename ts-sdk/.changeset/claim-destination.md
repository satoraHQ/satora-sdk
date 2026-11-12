---
"@satora/swap": patch
---

Claims pay out to the target the swap was created with: creation refuses a
response naming another target or claiming address, the target is stored
with the swap, and a server refresh can no longer change a stored swap's
payout addresses. SQLite storage now keeps the target and bridge recipient
(see the legacy SDK changeset).
