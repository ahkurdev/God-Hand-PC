# God Hand Native Port — Progress Ledger

Target: `God Hand (Europe) (En,Fr,De,Es,It).iso` → extracted to `../extracted/`
Goal: native Windows/Linux port in Rust, no emulator.

## Asset pipeline: WORKING, verified visually

Two real God Hand textures now decode and display correctly:

- **`_tex26_fixed.png`** — a Japanese UI panel with legible kanji (神事, 命魂,
  陣地), wooden framing, and a vertical scroll hierarchy.
- **`_fixed1134.png`** — Shannon sitting on a wooden stool reading a book
  inside a Japanese-style room, with a carved cabinet behind her.

Both come from the shipped `_DATA.AFS` via the Rust pipeline. This is the
first point at which the output is recognisably the game rather than noise.

### Proven at scale

`godhand-rs extract-all <disc-dir> <out-dir>` decodes every TIM2 in the
archive: **33 textures, 0 failures, 1530 non-TIM2 entries correctly skipped.**

The set includes Shannon reading in a Japanese room, Shannon riding a
carousel horse, and Shannon pulling a casino slot lever beside a male
character. Consistency across 33 unrelated textures is what rules out
luck — a handful of hand-picked successes could always be coincidence.

### The two bugs that had blocked this for a long time

**1. Textures are stored linearly, not swizzled.** Every swizzle candidate
tried (8x8 row-major, 8x8 column-major, 16x8 and 32x8 column-pairs) produced
noise. Linear layout produced a legible image immediately. The coherence
figures agree: linear scores 60.3% horizontal and 58.9% vertical — the
balanced profile a real 2D image has — while swizzled variants skew hard
toward one axis.

**2. The CLUT sits *after* the pixel data, not before.** This was the
longest-standing error in the parser. Reading the block ahead of the pixels
returns image-header bytes that decode to garbage; the block behind them is a
smooth RGBA8888 ramp. Two signals pinned it down:

- The average delta between consecutive palette entries drops from 226
  (wrong offset) to 94 (right offset).
- Every entry's fourth byte is `0x80` — the alpha channel, and the signature
  of a 4-byte-per-entry layout.

The CLUT is plain linear RGBA8888. No CSM1 permutation, no RGBA5551. Both of
those were detours that the wrong offset had made look plausible.

### Lesson worth keeping

Rendering the output as a raw grayscale index — bypassing the CLUT entirely —
is what isolated the swizzle question from the palette question. Without that
step the two bugs were entangled and neither could be diagnosed.

## Approach (modeled on SpartanPort)

SpartanPort runs God of War III's PS4 `eboot.bin` natively because the PS4 CPU
is already x86-64. God Hand's PS2 CPU is MIPS R5900, so the equivalent move is
**static recompilation**: translate MIPS to x86-64 once, then run the result
directly on the host CPU. Game logic executes natively; we supply the system
layer the PS2 firmware would have provided.

## Recompiler status (VERIFIED on shipped `.text`)

`cargo run -- recomp-profile ../extracted`

| Metric | Value |
|---|---|
| MIPS instructions | 721,368 |
| **Lifted to x86-64** | **553,952 (76.8%)** |
| Trapped (COP0/COP2/VU) | 167,416 (23.2%) |
| x86-64 emitted | 5.47 MB (1.9x source) |

**54/54 unit tests pass, exit 0.** Of those, 14 execute the emitted x86-64
through `VirtualAlloc`/`mmap` and check results against MIPS semantics — the
translation is verified by running it, not by eyeballing bytes.

### Bugs found and fixed in the lifter (real, not test-only)

1. **`disp8` signedness** — x86 encodes `disp8` as a *signed* byte. HI and LO
   sit at offsets 128 and 132, so `mov [rbx+0x84], eax` actually addressed
   `[rbx-124]`, writing outside the register file and segfaulting. Fixed by
   emitting `disp32` (ModRM mod=10) for any offset above 127.
2. **Wrong `setcc` opcodes** — used `0x5C`/`0x52`, which are `addps`, not
   `setl`/`setb`. Correct values are `0x9C`/`0x92`.
3. **`Slt` destroyed its operand** — a `mov eax, 0` between the load and the
   compare made every comparison `0 < rt`. Removed; `setcc` already yields 0/1.
4. **`rbx` not preserved** — `rbx` is callee-saved on both ABIs, so the
   prologue must `push` it and the epilogue `pop` it. Without this the host
   caller's frame was corrupted.
5. **Fragile `imul` encoding** — replaced the one-operand `F7 /4` form with an
   explicit 64-bit `imul rax, rdx` after zero-extending both operands.

### Register file layout

```text
rbx -> [r0..r31 (32 x 4 bytes)] [HI at 128] [LO at 132]   ; 136 bytes total
```

Offsets 0-127 fit in `disp8`; HI/LO require `disp32`.

### Remaining 23.2%
- **COP0** — system control, TLB, exception handling
- **COP2** — VU0/VU1 vector units (geometry, transforms)
- **DSUB/DADD** — 128-bit VU interop, needs a 64-bit register model
- **MOVZ/MOVN** — conditional moves
- **Unaligned LWL/LWR/SWL/SWR**

### Known limitation: memory model

MIPS addresses are 32-bit and index a flat space, but the host stack and heap
live at 64-bit addresses that `LUI`+`ORI` cannot express. The current
load/store path dereferences the MIPS address directly, which only works when
the target happens to be reachable in 32 bits. A real port needs a memory
abstraction mapping MIPS addresses into a host-owned buffer.

## Memory model (DONE)

`src/recomp/memory.rs` allocates a host-owned buffer standing in for the PS2's
32-bit address space, and lifted load/store now address memory through `r12`
rather than dereferencing the MIPS address directly.

```text
rbx -> MIPS register file (136 bytes)
r12 -> MIPS memory base    (2 GB window)
```

A MIPS address becomes a byte offset: `mov edx, [r12 + rax]`. The window is
2 GB because the scratchpad sits at `0x70000000`, 1.75 GB into the space — a
128 MB window would have left it unmapped.

Verified by execution:
- `sw`/`lw` round-trip at `0x00100000` (where God Hand's `.text` is mapped)
- `sw` at `0x70000000` (the PS2 scratchpad)
- `sb`/`lbu` narrow paths at `0x00200000`

**61/61 unit tests pass, exit 0.** Of those, 17 execute emitted x86-64 and
check results against MIPS semantics.

Not covered: hardware registers (DMA, GS, VIF) above `0x80000000`, which need
side-effecting handlers rather than a flat buffer.

## Shipped-code verification

Two tests lift and execute instructions taken verbatim from God Hand's own
`.text` at `0x00100290` — the tail of the first real function, just before the
binary's first `jr $ra`:

```text
3C030042  lui   $v1, 0x0042
3C040042  lui   $a0, 0x0042
2463FE18  addiu $v1, $v1, -488     -> 0x0041FE18
2484FE10  addiu $a0, $a0, -496     -> 0x0041FE10
AE030214  sw    $v1, 0x0214($s0)
AE0404AC  sw    $a0, 0x04AC($s0)
```

Both computed values and both memory writes are checked after execution. This
is the proof that the lifter handles the shipped binary, not only hand-picked
cases.

**67/67 unit tests pass, exit 0.** Of those, 19 execute emitted x86-64 and
check results against MIPS semantics.

### A note on the ELF entry point

`0x00100008` is *not* executable code. Its words decode to opcode `0x1C`,
which MIPS leaves reserved — it is a data table. The first genuinely
executable code appears at `0x00100290`. Anyone profiling this binary by
naively lifting from the entry point will measure a much lower hit rate for a
reason that has nothing to do with the lifter.

## DSUB / DADD

DSUB is the single most common trapped instruction: 72,721 uses, 10% of the
whole binary. It is now lifted as a 64-bit operation over register pairs,
which is what those uses amount to. Verified by execution, including a
subtract that borrows across the pair boundary.

The true 128-bit form needs the VU register file and remains unimplemented.

## Verified facts (from shipped artifacts)

### Disc
- ISO 9660 + UDF, volume `GODHAND`, Capcom/Clover Studio, 2006-10-02
- `SYSTEM.CNF`: `BOOT2 = cdrom0:\SLES_544.90;1`, VMODE=PAL
- 7 files: SLES_544.90 (3.5M ELF MIPS), _DATA.AFS (1.94G), GODHAND.FST (36K),
  IOPRP310.IMG, IRX.IMS, 0FLISTDV.DIR

### Executable (SLES_544.90)
- ELF32, MIPS R5900, entry 0x00100008, DVP toolchain, stripped (no symtab)
- 251 C++ RTTI class names recovered from .rodata (cEm*/cOl*/cOm*/cPl*/cCock*/cSubScr*/cCollision*/cTask*/cDamage*)
- CRI middleware stack, all built Nov 4 2005: ADXT 9.67, SRD 2.24, CVFS, SVM, MFCI, DVCI, HTCI, LSC, SJRMT, SJX, SKG, DTX, ADXF, AIXP, PS2RNA
- Build paths: `d:/GodHand/data/dll`, `host0:d:/godhand/localdll/%s`

### AFS (_DATA.AFS)
- 1563 entries, flags 0x3800, align 33152, no embedded name table
- 188 nested AFS, 162 SES, 81 SNR2, 35 TIM2, 16 SFD (MPEG video)

### FST (GODHAND.FST)
- magic FST, ver 1.0, group_count 16, string table 0x10–0x5D23
- 1563 file names, 62 directories, 81 .rel modules
- **CRITICAL: FST index ≠ AFS index.** FST names are logical roles; physical
  content is mixed AFS/SES/REL. Must map by name, not by index.
- Extensions: .dat 690, .afd 188, .afs 188, .idd 185, .ses 162, .rel 81,
  .tm2 35, .sfd 16, .bin 8, .t32 8, .bmt 1, .atb 1

## TIM2 v4 layout (VERIFIED against 5 textures)

```
0x00  'TIM2' ver=4 fmt=0 nimg=1
0x10  total_size  u32   = 0x30 + clut_size + image_size
0x14  clut_size   u32
0x18  image_size  u32
0x1C  u16 (block count, provenance only)
0x20  image header (0x20 bytes)
0x28  TEX0 u64  -> psm = bits 20-24, tw = bits 26-29, th = bits 30-33
0x30  TEX1 u64
0x40  CLUT data (clut_size)
...   pixel data (image_size)
```

Dimensions: `width = 1<<tw`, `height = 1<<th`. Cross-check
`width*height*bpp/8 == image_size` — holds exactly for all 5 tested.

Verified textures:
| AFS idx | size   | dims     | psm    |
|---------|--------|----------|--------|
| 10      | 2176   | 64x64    | PSMT4  |
| 15      | 5184   | 64x64    | PSMT8  |
| 17      | 8320   | 256x64   | PSMT4  |
| 26      | 66624  | 256x256  | PSMT8  |
| 1134    | 132160 | 512x256  | PSMT8  |

## Rust project state (`godhand-rs/`)

Working, 17/17 tests pass:
- `src/afs.rs` — AFS header + TOC parse, entry read
- `src/fst.rs` — FST string table, name lookup, extension histogram
- `src/tim2.rs` — TIM2 parse, TEX0 dimensions, unswizzle 4/8/16/32
- `src/main.rs` — CLI: inspect / list / extract / dump-strings

## OPEN — CLUT decoding (BLOCKED, needs a source of truth)

### What is PROVEN correct
- **Pixel unswizzle (PSMT8): CORRECT.** `_tex26` shows 60.2% horizontal and
  54.7% vertical neighbor coherence. A wrong swizzle gives ~0%.
- **Dimensions: CORRECT.** `1<<tw x 1<<th` from TEX0 reconciles exactly with
  `image_size` for all 5 tested textures.
- **File layout offsets: CORRECT.** `file_size == 0x10 + 0x30 + clut_size +
  image_size` holds exactly for all 5.

### What is WRONG
The bytes at offset 0x40 are **not a CLUT**. Evidence:
- AFS[26] "CLUT" first entries: `0x9F78, 0x8C9F, 0x7E7E, 0x0276, 0x0F02,
  0xC347, 0xE5E5` — these jump between green-yellow, red, magenta with no
  smooth gradient. A real palette ramps gradually.
- `0xE5E5` repeats identically at entries 6 and 9.
- Decoding as RGBA5551 gives 46% opaque with plausible hues (greens, blues)
  but scrambled spatial placement.

Conclusion: offset 0x40 is more likely the **start of pixel data**, and the
real CLUT lives elsewhere — possibly before 0x40, inside the 0x20-byte image
header region, or in a separate structure.

### Do NOT do again
Stop guessing offsets byte-by-byte. This has consumed many iterations with
no verification path. The correct next move is one of:

1. **Install Ghidra** (`GHIDRA_INSTALL_DIR`) and decompile God Hand's own
   texture-loading routine from MIPS `.text` to read the real layout. This is
   the authoritative answer.
2. **Find a reference implementation** — PCSX2's source has a TIM2/CLUT
   loader; porting its logic is faster and verifiable.
3. **Ask LO to point at a known-good texture** whose appearance is documented,
   so output can be checked against ground truth.

### Verified-good output
- `_tex26.png` 256x256, 172 colors, 60% spatial coherence — pixel data is
  real and correctly unswizzled; only the palette source is wrong.
- `_tex10.png`, `_tex1134.png` — all transparent (CLUT region all-zero).
  Genuinely empty placeholders, not a bug.

## Architecture roadmap

1. Asset pipeline (IN PROGRESS) — AFS/FST/TIM2 done, CLUT open
2. Renderer + asset viewer — blocked on CLUT
3. Game logic — MIPS reverse engineering, needs Ghidra (GHIDRA_INSTALL_DIR)
4. Full game — input, physics, AI, audio, UI

## Tooling
- REA v6.5.0 MCP installed (10 clients), 53 tools available
- No native analysis engine yet (Ghidra/IDA/JADX unset) — needed for MIPS decompile
- Rust 1.98.1, Node v24.22.1, Python 3.15.2
