# CLAUDE.md

Instructions for any AI agent continuing work on this repository.

## Read this first

`PROGRESS.md` is the ledger. It records what is verified, what is not, and —
most importantly — **the specific guesses that turned out wrong**. Read it
before touching the asset pipeline or the lifter. Several formats in this
project have plausible-looking wrong answers that cost days.

## Ground rules

1. **Evidence over assumption.** Every claim about a shipped format must come
   from the bytes, not from what the format "usually" looks like. When you
   find an answer, record the signal that pinned it down so the next agent
   does not re-derive it.

2. **Verify by running, not by reading.** The lifter has a test harness that
   executes emitted x86-64 and checks results against MIPS semantics
   (`src/recomp/exec.rs`). Use it. Five real bugs were found this way that
   byte-level inspection had missed.

3. **Render to see.** For anything visual, decode and *look* at the output.
   A coherence percentage can look fine while the image is noise. The
   grayscale-index trick in `PROGRESS.md` is how the swizzle and palette
   questions were separated.

4. **Never commit game data.** `../extracted/` is gitignored on purpose.

5. **Update `PROGRESS.md` when you learn something.** Especially when you
   disprove something. A recorded dead end is worth more than a recorded
   success.

## Build and test

```bash
cargo build --release
cargo test -- --test-threads=1
```

68 tests currently pass. Do not regress them.

## Where things live

```
src/
  afs.rs        CRI AFS archive reader — header, TOC, entry reads
  fst.rs        GODHAND.FST manifest — string table, name lookup
  tim2.rs       TIM2 v4 decoder — dimensions from TEX0, CLUT, unswizzle
  main.rs       CLI dispatch
  viewer.rs     Native window viewer (minifb)
  recomp/
    decode.rs   MIPS instruction decoder and classifier
    lifter.rs   MIPS → x86-64 translation
    memory.rs   MIPS 32-bit address space
    exec.rs     Execution harness — runs the emitted code
```

## Known traps

These are documented in `PROGRESS.md` in full. The short version:

- **TIM2 pixel data is linear, not swizzled.** Every swizzle candidate
  produces noise.
- **The TIM2 CLUT comes *after* the pixel data.** Reading it from before
  returns header bytes that look like a plausible palette but are not.
- **The ELF entry point is not code.** `0x00100008` decodes to reserved
  opcode `0x1C`; it is a data table. Real code starts at `0x00100290`.
- **x86 `disp8` is signed.** HI and LO sit at offsets 128 and 132 and need
  `disp32`. Getting this wrong writes outside the register file.
- **FST index is not AFS index.** The FST names entries by logical role, so a
  `.tm2` name may hold an AFS, a SES bank, or a REL module.

## If you add an instruction to the lifter

1. Add it to `SpecialFn`/`Op` and `classify` in `decode.rs`.
2. Implement the emission in `lifter.rs`.
3. Add an execution test in `exec.rs` that runs it and checks the result.
4. Re-run `recomp-profile` against the real disc and report the delta.

A lifted instruction with no execution test is not done.

## Style

Match the surrounding code. Doc comments explain *why*, not *what* — and when
a format was established from evidence, say what the evidence was. The
existing modules are the reference for tone and density.
