# T30 tail-rank divergence diagnostic

source: `/tmp/t30-probe.db`

## query: `embedding`

- legacy plan: `Factual`, outcome: `Ok`
- unified plan: `Factual`, outcome: `Ok`

### top-10 IDs

| rank | legacy | unified |
|---:|---|---|
| 1 | `722580fd` ✓ | `722580fd` |
| 2 | `e321c502` | `eea63f30` |
| 3 | `e72755fe` | `2c598d06` |
| 4 | `2c598d06` | `ec973244` |
| 5 | `15c7bb82` ✓ | `15c7bb82` |
| 6 | `eea63f30` | `4cbae92c` |
| 7 | `0a38b696` | `9aad870e` |
| 8 | `554a6e3e` | `f185c205` |
| 9 | `0328ed63` | `0a38b696` |
| 10 | `a3c94785` | `0be03f58` |

- only-legacy (5): ["554a6e3e", "e321c502", "a3c94785", "e72755fe", "0328ed63"]
- only-unified (5): ["4cbae92c", "f185c205", "9aad870e", "0be03f58", "ec973244"]

### legacy fusion candidates

(no trace — explain=true not honored?)

### unified fusion candidates

(no trace — explain=true not honored?)

---

## query: `graph`

- legacy plan: `Factual`, outcome: `Ok`
- unified plan: `Factual`, outcome: `Ok`

### top-10 IDs

| rank | legacy | unified |
|---:|---|---|
| 1 | `6dfff4fa` ✓ | `6dfff4fa` |
| 2 | `e09183d6` | `f2e8a3d4` |
| 3 | `5f3fcde9` | `e09183d6` |
| 4 | `9af8428f` | `5f3fcde9` |
| 5 | `d32e4d3e` | `9af8428f` |
| 6 | `bfb54037` | `a3e71f74` |
| 7 | `006a748d` | `d32e4d3e` |
| 8 | `a3e71f74` | `bfb54037` |
| 9 | `2a5b9f7e` | `796ab424` |
| 10 | `6973d326` | `58febb2b` |

- only-legacy (3): ["2a5b9f7e", "006a748d", "6973d326"]
- only-unified (3): ["796ab424", "58febb2b", "f2e8a3d4"]

### legacy fusion candidates

(no trace — explain=true not honored?)

### unified fusion candidates

(no trace — explain=true not honored?)

---

## query: `session compaction`

- legacy plan: `Factual`, outcome: `Ok`
- unified plan: `Factual`, outcome: `Ok`

### top-10 IDs

| rank | legacy | unified |
|---:|---|---|
| 1 | `55db8d5d` ✓ | `55db8d5d` |
| 2 | `9f72da84` ✓ | `9f72da84` |
| 3 | `f95e27e1` ✓ | `f95e27e1` |
| 4 | `468727f6` ✓ | `468727f6` |
| 5 | `997a7903` ✓ | `997a7903` |
| 6 | `4ab519b2` | `85405b0c` |
| 7 | `1c6ceae5` | `4ab519b2` |
| 8 | `ce3ce403` | `a11222f8` |
| 9 | `62314124` | `061b4fca` |
| 10 | `f9a341e0` | `62314124` |

- only-legacy (3): ["f9a341e0", "1c6ceae5", "ce3ce403"]
- only-unified (3): ["061b4fca", "85405b0c", "a11222f8"]

### legacy fusion candidates

(no trace — explain=true not honored?)

### unified fusion candidates

(no trace — explain=true not honored?)

---

## query: `semantic meaning`

- legacy plan: `Factual`, outcome: `Ok`
- unified plan: `Factual`, outcome: `Ok`

### top-10 IDs

| rank | legacy | unified |
|---:|---|---|
| 1 | `619ac013` ✓ | `619ac013` |
| 2 | `a12c651c` ✓ | `a12c651c` |
| 3 | `7886fc40` | `287aa24a` |
| 4 | `04b30035` ✓ | `04b30035` |
| 5 | `287aa24a` | `fb6435ff` |
| 6 | `fb6435ff` | `ccf3d117` |
| 7 | `7d52d967` ✓ | `7d52d967` |
| 8 | `ccf3d117` | `14079428` |
| 9 | `1d1778d6` | `53946e62` |
| 10 | `14079428` | `d07a5ec6` |

- only-legacy (2): ["1d1778d6", "7886fc40"]
- only-unified (2): ["53946e62", "d07a5ec6"]

### legacy fusion candidates

(no trace — explain=true not honored?)

### unified fusion candidates

(no trace — explain=true not honored?)

---

## query: `memory safety`

- legacy plan: `Factual`, outcome: `Ok`
- unified plan: `Factual`, outcome: `Ok`

### top-10 IDs

| rank | legacy | unified |
|---:|---|---|
| 1 | `d31b3906` ✓ | `d31b3906` |
| 2 | `e6bde91f` ✓ | `e6bde91f` |
| 3 | `68c71c5a` ✓ | `68c71c5a` |
| 4 | `8a0c705a` ✓ | `8a0c705a` |
| 5 | `08f7890e` ✓ | `08f7890e` |
| 6 | `4501fe66` ✓ | `4501fe66` |
| 7 | `7bd4cc5b` ✓ | `7bd4cc5b` |
| 8 | `c3ce8a66` ✓ | `c3ce8a66` |
| 9 | `204eace4` ✓ | `204eace4` |
| 10 | `a7ad9591` | `f9ce0a83` |

- only-legacy (1): ["a7ad9591"]
- only-unified (1): ["f9ce0a83"]

### legacy fusion candidates

(no trace — explain=true not honored?)

### unified fusion candidates

(no trace — explain=true not honored?)

---

