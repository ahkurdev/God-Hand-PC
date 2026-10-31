# Product Requirements Document — God Hand PC

## What this is

A native port of **God Hand** (PlayStation 2, PAL, SLES-544.90) to Windows and
Linux, written in Rust. No emulator: the game's own MIPS code is statically
recompiled to x86-64 and runs directly on the host CPU.

## The approach

The model is [SpartanPort](https://github.com/FelipePrado0/SpartanPort), which
runs God of War III's PS4 `eboot.bin` natively because that CPU is already
x86-64. The PS2's Emotion Engine is MIPS R5900, so the equivalent move here is
**static recompilation** rather than direct execution.

What gets replaced is the system layer the PS2 firmware would have provided:
memory, threading, graphics, audio, controller, saves. The game's own logic,
AI, physics and scripting execute natively.

## Requirements

### R1 — Asset pipeline (DONE)

Read the shipped disc artifacts directly and decode them for use on the host.

| Artifact | Format | Status |
|---|---|---|
| `_DATA.AFS` | CRI AFS archive | 1563 entries read |
| `GODHAND.FST` | Disc manifest | 1563 names, 81 `.rel` modules |
| `SLES_544.90` | MIPS ELF, DVP toolchain | Parsed, 251 C++ classes recovered |
| TIM2 textures | Sony TIM2 v4 | 33 decoded, verified visually |

Acceptance: textures decode to recognisable game art. Met — a Japanese UI
panel with legible kanji, and Shannon reading a book, riding a carousel horse,
and pulling a casino slot lever.

### R2 — MIPS to x86-64 recompiler (PARTIAL)

Translate the shipped `.text` to host machine code once, ahead of execution.

| Metric | Value |
|---|---|
| Instructions in `.text` | 721,368 |
| Lifted | 553,952 (76.8%) |
| Trapped | 167,416 (23.2%) |

Acceptance: emitted code runs and produces correct results. Met — 19 unit
tests execute the output through `VirtualAlloc`/`mmap` and check it against
MIPS semantics, including instructions lifted verbatim from the shipped
binary.

### R3 — MIPS memory model (DONE)

A host-owned buffer standing in for the PS2's 32-bit address space, addressed
through `r12`. 2 GB window so the scratchpad at `0x70000000` is reachable.

Acceptance: loads and stores work at arbitrary MIPS addresses. Met.

### R4 — Native viewer (DONE)

A window that displays decoded textures, to prove the pipeline reaches the
screen. Arrow keys step, Escape quits.

### R5 — Remaining work (NOT STARTED)

| Component | Scope | Notes |
|---|---|---|
| COP1 / FPU | ~20% of the binary | Physics, camera, transforms |
| COP2 / VU0 / VU1 | 2,621 direct + interop | Vector units, geometry. Needs a microcode interpreter |
| 64-bit register model | DSUB/DADD full form | Current lift covers the 64-bit case only |
| Hardware registers | Above `0x80000000` | DMA, GS, VIF. Need side-effecting handlers, not a flat buffer |
| Graphics backend | GS → Vulkan | The largest single piece |
| Audio backend | CRI ADX/SES | SES is a 37-entry index table referencing `se/*.afs` |
| Controller / saves | System layer | |

## Non-goals

- **Pixel-perfect emulation.** This is a port, not an emulator. Where the
  original relied on PS2 quirks, the port may differ.
- **Asset redistribution.** No game files are committed. Users supply their
  own dump.
- **Mod support.** Not planned.

## Constraints

- Rust 1.98 or newer.
- No copyrighted game data in the repository.
- Every claim about the shipped binary must be backed by evidence from it, not
  assumption. This project has a long history of plausible-looking guesses
  turning out wrong; see `PROGRESS.md` for the specific cases.
