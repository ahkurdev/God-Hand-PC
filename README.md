# God Hand PC

A native port of **God Hand** (PlayStation 2, SLES-544.90) to Windows and
Linux, written in Rust. No emulator: the game's own MIPS code is statically
recompiled to x86-64 and runs directly on the host CPU.

The approach follows [SpartanPort](https://github.com/FelipePrado0/SpartanPort),
which runs God of War III's PS4 binary natively because that CPU is already
x86-64. The PS2's Emotion Engine is MIPS R5900, so the equivalent move here is
static recompilation rather than direct execution. What gets replaced is the
system layer the PS2 firmware would have provided.

**No game files are included.** You need your own dump of the PAL disc.

## Status

| Component | State |
|---|---|
| AFS archive reader | Working — 1563 entries from the shipped disc |
| FST manifest reader | Working — 1563 names, 81 dynamic `.rel` modules |
| TIM2 texture decoder | Working — verified visually against real game art |
| MIPS → x86-64 lifter | 76.8% of the shipped `.text`, verified by execution |
| MIPS memory model | Working — 2 GB window, `r12`-based addressing |
| Execution harness | Working — 68 unit tests, 19 of which run emitted code |

See [PROGRESS.md](PROGRESS.md) for the detailed ledger, including the bugs
that were found and how.

## Building

Requires Rust 1.98 or newer.

```bash
cargo build --release
```

## Usage

Point the tools at a directory holding the extracted disc files
(`_DATA.AFS`, `GODHAND.FST`, `SLES_544.90`):

```bash
# Report disc, AFS and FST structure
godhand-rs inspect <disc-dir>

# List AFS entries with their FST names, optionally filtered
godhand-rs list <disc-dir> [filter]

# Decode one AFS entry as a TIM2 texture to PNG
godhand-rs extract <disc-dir> <index> <out.png>

# Printable strings from an AFS entry
godhand-rs dump-strings <disc-dir> <index>

# Measure how much of the shipped .text the lifter covers
godhand-rs recomp-profile <disc-dir>
```

## Layout

```
src/
  afs.rs        CRI AFS archive reader
  fst.rs        GODHAND.FST manifest reader
  tim2.rs       Sony TIM2 texture decoder
  main.rs       CLI entry point
  recomp/
    decode.rs   MIPS R5900 instruction decoder
    lifter.rs   MIPS → x86-64 translation
    memory.rs   MIPS 32-bit address space
    exec.rs     Execution harness that runs the emitted code
```

## What is not done yet

- **COP1/FPU** — roughly 20% of the binary
- **COP2 / VU0 / VU1** — vector units, geometry and transforms
- **Hardware registers** above `0x80000000` (DMA, GS, VIF), which need
  side-effecting handlers rather than a flat buffer
- **System layer** — graphics, audio, controller, saves

## Acknowledgements

- [SpartanPort](https://github.com/FelipePrado0/SpartanPort) for the native-port
  approach this project follows.
- Capcom and Clover Studio for God Hand.
