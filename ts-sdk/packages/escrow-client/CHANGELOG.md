# @satora/escrow-client

## 8.0.0

### Patch Changes

- Updated dependencies [6721be5]
- Updated dependencies [6c428fc]
- Updated dependencies [151af66]
  - @satora/swap@1.4.0

## 7.0.3

### Patch Changes

- Updated dependencies [f401036]
- Updated dependencies [b8e0b74]
  - @satora/swap@1.3.3

## 7.0.2

### Patch Changes

- 0f7f94f: Target backend 0.3.10 in the x-satora-server-version header and align the
  Arkade SDK dependency on 0.4.66.
- Updated dependencies [0f7f94f]
- Updated dependencies [6817e60]
  - @satora/swap@1.3.2
  - @satora/escrow@0.0.6

## 7.0.1

### Patch Changes

- Updated dependencies [e198470]
- Updated dependencies [7771b28]
- Updated dependencies [1997ca0]
  - @satora/swap@1.3.1

## 7.0.0

### Patch Changes

- Updated dependencies [bc8011c]
- Updated dependencies [8ef73bb]
  - @satora/swap@1.3.0

## 6.0.1

### Patch Changes

- Updated dependencies [36fab34]
- Updated dependencies [493a61a]
  - @satora/swap@1.2.1

## 6.0.0

### Patch Changes

- Updated dependencies [7678afc]
  - @satora/swap@1.2.0

## 5.0.0

### Patch Changes

- Updated dependencies [d50ee09]
- Updated dependencies [f96e1f3]
- Updated dependencies [9f3bb60]
- Updated dependencies [e389209]
  - @satora/swap@1.1.0

## 4.0.0

### Major Changes

- 6866996: Lightning v2 (Spark provider), clean wire break.

  - `@satora/swap`: Lightning→Arkade tracking now reads the generic
    `target_amount` field; wrappers for the removed Arkade→Lightning and
    Lightning↔EVM directions are gone until those flows are rebuilt on the
    new provider.
  - `@satora/escrow-client`: `fundFromLightning` uses `targetAmountSats`;
    `withdrawToLightning` / `quoteLightningWithdrawal` keep their signatures
    but throw `LightningWithdrawalUnavailableError` while Arkade→Lightning
    swaps are rebuilt.

### Patch Changes

- Updated dependencies [64e8902]
- Updated dependencies [8dbb24f]
- Updated dependencies [b77fbf9]
- Updated dependencies [1068ea3]
- Updated dependencies [473eac2]
- Updated dependencies [6866996]
- Updated dependencies [e305ec8]
  - @satora/swap@1.0.0

## 3.0.2

### Patch Changes

- Updated dependencies [9153ac2]
  - @satora/swap@0.3.2

## 3.0.1

### Patch Changes

- Updated dependencies [75f4743]
  - @satora/swap@0.3.1

## 3.0.0

### Patch Changes

- Updated dependencies [573eec6]
- Updated dependencies [31484e1]
- Updated dependencies [1c027f8]
  - @satora/swap@0.3.0

## 2.0.0

### Patch Changes

- Updated dependencies [e2271fe]
- Updated dependencies [9d35eee]
  - @satora/swap@0.2.0

## 1.0.0

### Patch Changes

- Updated dependencies [1db87ef]
- Updated dependencies [80b3047]
  - @satora/swap@0.1.0

## 0.0.5

### Patch Changes

- Updated dependencies [9f4d595]
- Updated dependencies [bbba274]
  - @satora/escrow@0.0.5
  - @satora/swap@0.0.5

## 0.0.5-rc.0

### Patch Changes

- Updated dependencies [9f4d595]
- Updated dependencies [bbba274]
  - @satora/escrow@0.0.5-rc.0
  - @satora/swap@0.0.5-rc.0
