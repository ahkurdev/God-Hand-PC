# Output

What this project has actually produced, with the evidence for each claim.

## 1. Disc structure, fully mapped

From `God Hand (Europe) (En,Fr,De,Es,It).iso` — Capcom / Clover Studio,
created 2006-10-02, PAL.

```
SYSTEM.CNF      BOOT2 = cdrom0:\SLES_544.90;1, VMODE=PAL
SLES_544.90     3.5 MB   MIPS R5900 ELF, DVP toolchain, stripped
_DATA.AFS       1.94 GB  1563 assets
GODHAND.FST     36 KB    manifest
IOPRP310.IMG    272 KB   IOP reset image
IRX.IMS         307 KB   IOP modules
0FLISTDV.DIR    23 B     file list
```

`SYSTEM.CNF` is 56 bytes and names the executable directly, which is how the
boot chain was confirmed rather than inferred.

## 2. The executable's architecture, recovered

**251 C++ class names** pulled from `.rodata` as RTTI strings, even though the
symbol table is stripped:

| Prefix | Meaning | Count |
|---|---|---|
| `cEm*` | Enemies | ~30 |
| `cOl*` | Level objects | ~70 |
| `cOm*` | Map objects | ~110 |
| `cPl*` | Player | 2 |
| `cCock*` | HUD | ~15 |
| `cSubScr*` | Sub-screens | ~12 |
| `cCollision*` | Collision | ~15 |
| `cTask*` / `cDamage*` | Task system, combat | ~6 |

Signature features confirmed present: `cHeatSys` (the Heat gauge),
`cSubScrGodReel` (God Reel), `cOmTrump` (Trump cards), `cSubScrShopMain`.

**CRI middleware stack**, all built 2005-11-04: ADXT 9.67, SRD 2.24, CVFS,
SVM, MFCI, DVCI, HTCI, LSC, SJRMT, SJX, SKG, DTX, ADXF, AIXP, PS2RNA.

**Build paths** recovered from strings: `d:/GodHand/data/dll`,
`host0:d:/godhand/localdll/%s`, `McPort.rel`, `r%x%02x.rel`.

## 3. The asset manifest

`GODHAND.FST` names all 1563 assets across 67 directories:

| Extension | Count | What it is |
|---|---|---|
| `.dat` | 690 | Stage and object data |
| `.afd` / `.afs` | 188 / 188 | Audio descriptor + nested archive |
| `.idd` | 185 | UI definitions |
| `.ses` | 162 | Sound effect sets |
| `.rel` | 81 | Dynamic code modules |
| `.tm2` | 35 | Textures |
| `.sfd` | 16 | FMV video (MPEG-PS) |
| `.bin` / `.t32` | 8 / 8 | Localisation, textures |

The `.idd` names enumerate the game's content directly: `idblackjack`,
`idpoker`, `idslot`, `idcolosseum`, `iddograce`, `idsubscrgodreel`,
`idwantedlist` — the minigames and menus are all there.

Localisation covers seven languages: `core_jpn`, `core_usa`, `core_eng`,
`core_ger`, `core_fra`, `core_spa`, `core_ita`.

## 4. Textures, decoded and displayed

**33 textures, 0 failures** from `godhand-rs extract-all`. Three inspected by
eye:

- A Japanese UI panel — kanji legible (神事, 命魂, 陣地), wooden framing,
  vertical scroll hierarchy
- Shannon reading a book in a Japanese room, carved cabinet behind her
- Shannon riding a carousel horse, gold ornament and round lamps
- Shannon pulling a casino slot lever, reels with skull symbols and "BAR"

Consistency across 33 unrelated textures is what rules out luck.

## 5. A working MIPS to x86-64 recompiler

```
instructions : 721,368
lifted       : 553,952  (76.8%)
trapped      : 167,416  (23.2%)
x86-64 output: 5.79 MB  (2.5x source)
```

**68 unit tests pass**, 19 of which execute the emitted machine code and check
results against MIPS semantics. Two of those run instructions lifted verbatim
from the shipped `.text` at `0x00100290`.

Coverage of the shipped instruction mix:

| Opcode | Share of `.text` | Lifted |
|---|---|---|
| SPECIAL | 23.1% | Yes |
| ADDIU | 14.0% | Yes |
| LW | 8.0% | Yes |
| JAL | 5.6% | Yes |
| BEQ | 5.5% | Yes |
| LUI | 4.7% | Yes |
| SW | 3.9% | Yes |
| DSUB | 10.1% | Yes (64-bit form) |
| COP1 / VU | ~20% | No |

## 6. Bugs found and fixed

All of these were found by *running* the output, not by reading the bytes:

1. **`disp8` signedness** — HI/LO at offsets 128/132 became negative, writing
   outside the register file and segfaulting.
2. **Wrong `setcc` opcodes** — used `0x5C`/`0x52`, which are `addps`.
3. **`Slt` destroyed its own operand** — a `mov eax, 0` before the compare
   made every comparison `0 < rt`.
4. **`rbx` not preserved** — callee-saved register clobbered, corrupting the
   host caller's frame.
5. **Fragile `imul` encoding** — replaced with an explicit 64-bit form.
6. **Memory window too small** — 128 MB left the scratchpad at `0x70000000`
   unmapped.
7. **TIM2 textures are linear, not swizzled.**
8. **The TIM2 CLUT sits after the pixel data, not before.**

## 7. A native viewer

`godhand-rs view <disc-dir>` opens a window and steps through all 33 textures.
`view-list` does the same enumeration headlessly, which is what makes the
viewer testable on machines without a display.

## What is not done

- COP1/FPU (~20% of the binary)
- COP2 / VU0 / VU1 vector units
- Hardware registers above `0x80000000`
- Graphics, audio, controller, saves
- Any actual gameplay

See `PRD.md` R5 for the full table.
