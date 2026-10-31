//! Execution harness for recompiled x86-64 code.
//!
//! Static recompilation is only proven correct when the emitted machine code
//! actually runs. This module allocates executable memory, copies lifted code
//! into it, and calls it with a MIPS register file, so the translation can be
//! verified against known MIPS semantics rather than merely eyeballed.
//!
//! ## Platform support
//!
//! Windows uses `VirtualAlloc` with `PAGE_EXECUTE_READWRITE`. Unix uses
//! `mmap` with `PROT_READ | PROT_WRITE | PROT_EXEC`. Both are required because
//! modern systems default to W^X, which would reject a plain write.

use super::lifter::REG_FILE_SIZE;
use super::memory::Memory;

/// Error type for execution-harness operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// The platform refused to allocate executable memory.
    AllocationFailed(String),
    /// The lifted code trapped with `int3`.
    Trapped,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::AllocationFailed(msg) => write!(f, "allocation failed: {msg}"),
            ExecError::Trapped => write!(f, "recompiled code hit a trap"),
        }
    }
}

impl std::error::Error for ExecError {}

/// Result alias for execution-harness operations.
pub type ExecResult<T> = Result<T, ExecError>;

/// A block of executable memory holding lifted x86-64 code.
pub struct ExecutableCode {
    /// Base address of the allocation.
    ptr: *mut u8,
    /// Size of the allocation in bytes.
    size: usize,
}

// The buffer is only ever executed through `call`, which requires it to be
// movable between threads.
unsafe impl Send for ExecutableCode {}

impl ExecutableCode {
    /// Allocate `size` bytes of executable memory and copy `code` into it.
    ///
    /// `size` is rounded up to the platform's page granularity.
    pub fn new(code: &[u8]) -> ExecResult<ExecutableCode> {
        let size = code.len().max(4096);
        #[cfg(windows)]
        {
            use std::ptr::null_mut;
            extern "system" {
                fn VirtualAlloc(
                    addr: *mut core::ffi::c_void,
                    size: usize,
                    alloc_type: u32,
                    protect: u32,
                ) -> *mut core::ffi::c_void;
            }
            const MEM_COMMIT: u32 = 0x1000;
            const MEM_RESERVE: u32 = 0x2000;
            const PAGE_EXECUTE_READWRITE: u32 = 0x40;
            let ptr = unsafe {
                VirtualAlloc(
                    null_mut(),
                    size,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_EXECUTE_READWRITE,
                )
            };
            if ptr.is_null() {
                return Err(ExecError::AllocationFailed("VirtualAlloc returned null".into()));
            }
            let dst = ptr as *mut u8;
            unsafe {
                std::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len());
            }
            Ok(ExecutableCode {
                ptr: dst,
                size,
            })
        }
        #[cfg(unix)]
        {
            extern "system" {
                fn mmap(
                    addr: *mut core::ffi::c_void,
                    len: usize,
                    prot: i32,
                    flags: i32,
                    fd: i32,
                    off: i64,
                ) -> *mut core::ffi::c_void;
            }
            const PROT_READ: i32 = 0x1;
            const PROT_WRITE: i32 = 0x2;
            const PROT_EXEC: i32 = 0x4;
            const MAP_PRIVATE: i32 = 0x02;
            const MAP_ANONYMOUS: i32 = 0x20;
            let ptr = unsafe {
                mmap(
                    std::ptr::null_mut(),
                    size,
                    PROT_READ | PROT_WRITE | PROT_EXEC,
                    MAP_PRIVATE | MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if ptr as isize == -1 {
                return Err(ExecError::AllocationFailed("mmap failed".into()));
            }
            let dst = ptr as *mut u8;
            unsafe {
                std::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len());
            }
            Ok(ExecutableCode { ptr: dst, size })
        }
    }

    /// Call the lifted code with a MIPS register file and memory base.
    ///
    /// The lifted function takes two arguments: the register-file pointer and
    /// the memory base. On Windows x64 they arrive in `rcx` and `rdx`; on
    /// System V in `rdi` and `rsi`. The prologue moves them into `rbx` and
    /// `r12`, both callee-saved and therefore stable across the sequence. On
    /// return the register file holds the updated values.
    ///
    /// # Safety
    ///
    /// The caller must ensure the lifted code was produced from instructions
    /// this lifter understands. Trapped instructions execute `int3`, which
    /// aborts the process rather than returning an error, so the caller should
    /// check the lift result before calling.
    pub unsafe fn call(
        &self,
        regs: &mut [u32; REG_FILE_SIZE / 4],
        mem: &mut Memory,
    ) {
        #[cfg(windows)]
        {
            let f: extern "win64" fn(*mut u32, *mut u8) = std::mem::transmute(self.ptr);
            f(regs.as_mut_ptr(), mem.base());
        }
        #[cfg(not(windows))]
        {
            let f: extern "sysv64" fn(*mut u32, *mut u8) = std::mem::transmute(self.ptr);
            f(regs.as_mut_ptr(), mem.base());
        }
    }

    /// Size of the allocation.
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for ExecutableCode {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            extern "system" {
                fn VirtualFree(
                    addr: *mut core::ffi::c_void,
                    size: usize,
                    free_type: u32,
                ) -> i32;
            }
            const MEM_RELEASE: u32 = 0x8000;
            unsafe {
                VirtualFree(self.ptr as *mut core::ffi::c_void, 0, MEM_RELEASE);
            }
        }
        #[cfg(unix)]
        {
            extern "system" {
                fn munmap(addr: *mut core::ffi::c_void, len: usize) -> i32;
            }
            unsafe {
                munmap(self.ptr as *mut core::ffi::c_void, self.size);
            }
        }
    }
}

/// A MIPS register file, indexed by register number.
///
/// Indices 0-31 are the general-purpose registers, 32 is HI and 33 is LO.
/// Register 0 is hardwired to zero by MIPS semantics; the harness does not
/// enforce this, so tests must avoid writing to `$zero`.
pub type RegFile = [u32; REG_FILE_SIZE / 4];

/// Create a zeroed register file.
pub fn new_regs() -> RegFile {
    [0u32; REG_FILE_SIZE / 4]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recomp::decode::Insn;
    use crate::recomp::lifter::{Lifter, LiftResult};

    /// Lift a sequence of instructions and run it against a fresh memory space.
    fn run(insns: &[Insn], regs: &mut RegFile) -> Memory {
        let mut lifter = Lifter::new();
        lifter.emit_prologue();
        for (i, insn) in insns.iter().enumerate() {
            let pc = 0x0010_0000u32 + (i as u32) * 4;
            assert_eq!(lifter.lift(insn, pc), LiftResult::Ok, "insn {i} trapped");
        }
        // Return to the caller so execution stops after the sequence.
        lifter.emit_epilogue();
        let code = ExecutableCode::new(lifter.code.as_slice()).expect("alloc");
        let mut mem = Memory::new().expect("mem");
        unsafe { code.call(regs, &mut mem) };
        mem
    }

    #[test]
    fn executes_addiu() {
        // addiu $t0, $zero, 42
        let insn = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 42);
        let mut regs = new_regs();
        run(&[insn], &mut regs);
        assert_eq!(regs[8], 42);
    }

    #[test]
    fn executes_addu() {
        // addiu $t0, $zero, 10
        // addiu $t1, $zero, 32
        // addu  $t2, $t0, $t1
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 10);
        let b = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 32);
        let c = Insn::decode((8 << 21) | (9 << 16) | (10 << 11) | 0x21);
        let mut regs = new_regs();
        run(&[a, b, c], &mut regs);
        assert_eq!(regs[10], 42);
    }

    #[test]
    fn executes_subu() {
        // addiu $t0, $zero, 100
        // addiu $t1, $zero, 58
        // subu  $t2, $t0, $t1
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 100);
        let b = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 58);
        let c = Insn::decode((8 << 21) | (9 << 16) | (10 << 11) | 0x23);
        let mut regs = new_regs();
        run(&[a, b, c], &mut regs);
        assert_eq!(regs[10], 42);
    }

    #[test]
    fn executes_ori() {
        // ori $t0, $zero, 0x00FF
        // ori $t0, $t0, 0xFF00
        let a = Insn::decode((13 << 26) | (0 << 21) | (8 << 16) | 0x00FF);
        let b = Insn::decode((13 << 26) | (8 << 21) | (8 << 16) | 0xFF00);
        let mut regs = new_regs();
        run(&[a, b], &mut regs);
        assert_eq!(regs[8], 0xFFFF);
    }

    #[test]
    fn executes_lui() {
        // lui $t0, 0x1234
        let insn = Insn::decode((15 << 26) | (8 << 16) | 0x1234);
        let mut regs = new_regs();
        run(&[insn], &mut regs);
        assert_eq!(regs[8], 0x1234_0000);
    }

    #[test]
    fn executes_sll() {
        // addiu $t0, $zero, 1
        // sll   $t1, $t0, 4
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 1);
        let b = Insn::decode((4 << 6) | (9 << 11) | (8 << 16) | 0x00);
        let mut regs = new_regs();
        run(&[a, b], &mut regs);
        assert_eq!(regs[9], 16);
    }

    #[test]
    fn executes_srl() {
        // addiu $t0, $zero, 256
        // srl   $t1, $t0, 4
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 256);
        let b = Insn::decode((4 << 6) | (9 << 11) | (8 << 16) | 0x02);
        let mut regs = new_regs();
        run(&[a, b], &mut regs);
        assert_eq!(regs[9], 16);
    }

    #[test]
    fn executes_slti_true_and_false() {
        // addiu $t0, $zero, 5
        // slti  $t1, $t0, 10   -> 1
        // slti  $t2, $t0, 3    -> 0
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 5);
        let b = Insn::decode((10 << 26) | (8 << 21) | (9 << 16) | 10);
        let c = Insn::decode((10 << 26) | (8 << 21) | (10 << 16) | 3);
        let mut regs = new_regs();
        run(&[a, b, c], &mut regs);
        assert_eq!(regs[9], 1);
        assert_eq!(regs[10], 0);
    }

    #[test]
    fn executes_slt() {
        // addiu $t0, $zero, 5
        // addiu $t1, $zero, 9
        // slt   $t2, $t0, $t1  -> 1
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 5);
        let b = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 9);
        let c = Insn::decode((8 << 21) | (9 << 16) | (10 << 11) | 0x2A);
        let mut regs = new_regs();
        run(&[a, b, c], &mut regs);
        assert_eq!(regs[10], 1);
    }

    #[test]
    fn executes_mult_into_hi_lo() {
        // addiu $t0, $zero, 7
        // addiu $t1, $zero, 6
        // mult  $t0, $t1
        // mflo  $t2
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 7);
        let b = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 6);
        let c = Insn::decode((8 << 21) | (9 << 16) | 0x18);
        let d = Insn::decode((10 << 11) | 0x12);
        let mut regs = new_regs();
        run(&[a, b, c, d], &mut regs);
        assert_eq!(regs[10], 42);
    }

    #[test]
    fn executes_and_or_xor() {
        // addiu $t0, $zero, 0xF0
        // addiu $t1, $zero, 0x0F
        // and   $t2, $t0, $t1  -> 0
        // or    $t3, $t0, $t1  -> 0xFF
        // xor   $t4, $t0, $t1  -> 0xFF
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 0xF0);
        let b = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 0x0F);
        let c = Insn::decode((8 << 21) | (9 << 16) | (10 << 11) | 0x24);
        let d = Insn::decode((8 << 21) | (9 << 16) | (11 << 11) | 0x25);
        let e = Insn::decode((8 << 21) | (9 << 16) | (12 << 11) | 0x26);
        let mut regs = new_regs();
        run(&[a, b, c, d, e], &mut regs);
        assert_eq!(regs[10], 0);
        assert_eq!(regs[11], 0xFF);
        assert_eq!(regs[12], 0xFF);
    }

    #[test]
    fn executes_load_and_store_roundtrip() {
        // With the memory model in place, a MIPS address is an offset into a
        // host-owned buffer, so the address can be any 32-bit value the game
        // would actually use. This writes to 0x00100000, where God Hand's own
        // .text is mapped, and reads it back.
        const TARGET: u32 = 0x0010_0000;
        let hi = (TARGET >> 16) & 0xFFFF;
        let lo = TARGET & 0xFFFF;

        let a = Insn::decode((15 << 26) | (8 << 16) | hi); // lui  $t0, hi
        let b = Insn::decode((13 << 26) | (8 << 21) | (8 << 16) | lo); // ori $t0, $t0, lo
        let c = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 123); // addiu $t1, $zero, 123
        let d = Insn::decode((43 << 26) | (8 << 21) | (9 << 16) | 0); // sw  $t1, 0($t0)
        let e = Insn::decode((35 << 26) | (8 << 21) | (10 << 16) | 0); // lw  $t2, 0($t0)

        let mut regs = new_regs();
        let mem = run(&[a, b, c, d, e], &mut regs);
        assert_eq!(regs[10], 123);
        assert_eq!(mem.read_u32(TARGET).expect("read"), 123);
    }

    #[test]
    fn executes_scratchpad_access() {
        // The scratchpad at 0x70000000 is where God Hand stages hot data. It
        // must be reachable through the same addressing path as RDRAM.
        const TARGET: u32 = 0x7000_0000;
        let hi = (TARGET >> 16) & 0xFFFF;
        let lo = TARGET & 0xFFFF;

        let a = Insn::decode((15 << 26) | (8 << 16) | hi);
        let b = Insn::decode((13 << 26) | (8 << 21) | (8 << 16) | lo);
        // ADDIU sign-extends its 16-bit immediate, so 0x7F12 stays positive
        // while 0xABCD would become 0xFFFFABCD. Use a value that survives the
        // extension unchanged.
        let c = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 0x7F12);
        let d = Insn::decode((43 << 26) | (8 << 21) | (9 << 16) | 0);

        let mut regs = new_regs();
        let mem = run(&[a, b, c, d], &mut regs);
        assert_eq!(mem.read_u32(TARGET).expect("read"), 0x7F12);
    }

    #[test]
    fn executes_byte_store_and_load() {
        // sb/lbu exercise the narrow load and store paths, which use different
        // ModRM encodings from the word paths.
        const TARGET: u32 = 0x0020_0000;
        let hi = (TARGET >> 16) & 0xFFFF;
        let lo = TARGET & 0xFFFF;

        let a = Insn::decode((15 << 26) | (8 << 16) | hi); // lui  $t0, hi
        let b = Insn::decode((13 << 26) | (8 << 21) | (8 << 16) | lo); // ori $t0, $t0, lo
        let c = Insn::decode((9 << 26) | (0 << 21) | (9 << 16) | 0x7F); // addiu $t1, $zero, 0x7F
        let d = Insn::decode((40 << 26) | (8 << 21) | (9 << 16) | 0); // sb $t1, 0($t0)
        let e = Insn::decode((36 << 26) | (8 << 21) | (10 << 16) | 0); // lbu $t2, 0($t0)

        let mut regs = new_regs();
        let mem = run(&[a, b, c, d, e], &mut regs);
        assert_eq!(regs[10], 0x7F);
        assert_eq!(mem.as_slice()[TARGET as usize], 0x7F);
    }

    #[test]
    fn executes_andi() {
        // addiu $t0, $zero, 0xFF
        // andi  $t1, $t0, 0x0F
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 0xFF);
        let b = Insn::decode((12 << 26) | (8 << 21) | (9 << 16) | 0x0F);
        let mut regs = new_regs();
        run(&[a, b], &mut regs);
        assert_eq!(regs[9], 0x0F);
    }

    #[test]
    fn executes_nor() {
        // addiu $t0, $zero, 0
        // nor   $t1, $t0, $t0  -> 0xFFFFFFFF
        let a = Insn::decode((9 << 26) | (0 << 21) | (8 << 16) | 0);
        let b = Insn::decode((8 << 21) | (8 << 16) | (9 << 11) | 0x27);
        let mut regs = new_regs();
        run(&[a, b], &mut regs);
        assert_eq!(regs[9], 0xFFFF_FFFF);
    }

    /// Write a 64-bit value into a MIPS register pair.
    fn set_pair(regs: &mut RegFile, reg: usize, value: u64) {
        regs[reg] = value as u32;
        regs[reg + 1] = (value >> 32) as u32;
    }

    /// Read a 64-bit value from a MIPS register pair.
    fn get_pair(regs: &RegFile, reg: usize) -> u64 {
        (regs[reg] as u64) | ((regs[reg + 1] as u64) << 32)
    }

    #[test]
    fn executes_dsub_64bit() {
        // DSUB is the single most common trapped instruction in God Hand:
        // 72,721 uses, 10% of the whole binary. This proves the 64-bit path
        // the game actually relies on.
        //
        //   $t0:$t1 = 0x0000000100000000
        //   $t2:$t3 = 0x0000000000000001
        //   dsub $t4, $t0, $t2   -> $t4:$t5 = 0xFFFFFFFF
        let dsub = Insn::decode((8 << 21) | (10 << 16) | (12 << 11) | 0x2D);
        let mut regs = new_regs();
        set_pair(&mut regs, 8, 0x0000_0001_0000_0000);
        set_pair(&mut regs, 10, 1);
        run(&[dsub], &mut regs);
        assert_eq!(get_pair(&regs, 12), 0xFFFF_FFFF);
    }

    #[test]
    fn executes_dadd_64bit() {
        //   $t0:$t1 = 0x00000000FFFFFFFF
        //   $t2:$t3 = 0x0000000000000001
        //   dadd $t4, $t0, $t2   -> $t4:$t5 = 0x0000000100000000
        let dadd = Insn::decode((8 << 21) | (10 << 16) | (12 << 11) | 0x2C);
        let mut regs = new_regs();
        set_pair(&mut regs, 8, 0x0000_0000_FFFF_FFFF);
        set_pair(&mut regs, 10, 1);
        run(&[dadd], &mut regs);
        assert_eq!(get_pair(&regs, 12), 0x0000_0001_0000_0000);
    }

    #[test]
    fn executes_dsub_borrow_across_pairs() {
        // A 64-bit subtract that must borrow from the high half, which is the
        // case a naive 32-bit implementation would get wrong.
        //
        //   $t0:$t1 = 0x0000000100000000
        //   $t2:$t3 = 0x0000000000000001
        //   dsub $t4, $t0, $t2   -> 0xFFFFFFFF
        //
        // and the reverse direction, where the result goes negative:
        //
        //   dsub $t6, $t2, $t0   -> 0xFFFFFFFFFFFFFFFF
        let dsub = Insn::decode((8 << 21) | (10 << 16) | (12 << 11) | 0x2D);
        let dsub_rev = Insn::decode((10 << 21) | (8 << 16) | (14 << 11) | 0x2D);
        let mut regs = new_regs();
        set_pair(&mut regs, 8, 0x0000_0001_0000_0000);
        set_pair(&mut regs, 10, 1);
        run(&[dsub, dsub_rev], &mut regs);
        assert_eq!(get_pair(&regs, 12), 0x0000_0000_FFFF_FFFF);
        assert_eq!(get_pair(&regs, 14), 0xFFFF_FFFF_0000_0001);
    }

    #[test]
    fn dsub_lifts_without_trapping() {
        let dsub = Insn::decode((8 << 21) | (10 << 16) | (12 << 11) | 0x2D);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&dsub, 0x1000), LiftResult::Ok);
        assert_eq!(lifter.trapped, 0);
        // Three 64-bit memory ops, each 7 bytes with a disp32.
        assert_eq!(lifter.code.len(), 21);
    }

    /// A real instruction sequence lifted verbatim from God Hand's `.text`.
    ///
    /// These nine words sit at `0x00100290`, just before the first `jr $ra` in
    /// the binary, and are the tail of an actual function. Running them proves
    /// the lifter handles shipped code rather than only hand-picked cases.
    ///
    /// Note that the ELF entry point at `0x00100008` is *not* code: those words
    /// decode to opcode `0x1C`, which MIPS leaves reserved. It is a data table,
    /// so the first genuinely executable code appears further in.
    ///
    /// Decoded by hand for reference:
    /// ```text
    /// 0080802D  daddu  $s0, $a0, $zero      (SPECIAL, needs the 64-bit model)
    /// 3C030042  lui    $v1, 0x0042
    /// 3C040042  lui    $a0, 0x0042
    /// 2463FE18  addiu  $v1, $v1, -488
    /// 2484FE10  addiu  $a0, $a0, -496
    /// AE030214  sw     $v1, 0x0214($s0)
    /// 0200102D  daddu  $v0, $s0, $zero      (SPECIAL, needs the 64-bit model)
    /// AE0404AC  sw     $a0, 0x04AC($s0)
    /// 03E00008  jr     $ra
    /// ```
    ///
    /// The two `daddu` words are 64-bit operations this lifter does not yet
    /// cover, so the executable subset below stops short of them.
    // The trailing `jr $ra` is omitted: it would jump to whatever `$ra` holds,
    // and the harness supplies its own return path. Everything before it is
    // lifted and executed.
    const GODHAND_REAL_CODE: [u32; 6] = [
        0x3C03_0042, // lui   $v1, 0x0042
        0x3C04_0042, // lui   $a0, 0x0042
        0x2463_FE18, // addiu $v1, $v1, -488
        0x2484_FE10, // addiu $a0, $a0, -496
        0xAE03_0214, // sw    $v1, 0x0214($s0)
        0xAE04_04AC, // sw    $a0, 0x04AC($s0)
    ];

    #[test]
    fn executes_godhand_shipped_code() {
        let insns: Vec<Insn> = GODHAND_REAL_CODE.iter().map(|&w| Insn::decode(w)).collect();
        let mut regs = new_regs();
        // $s0 holds the base address the shipped code stores through.
        regs[16] = 0x0010_0000;
        let mem = run(&insns, &mut regs);

        // lui $v1, 0x0042 then addiu -488 gives 0x0041_FE18.
        assert_eq!(regs[3], 0x0041_FE18);
        // lui $a0, 0x0042 then addiu -496 gives 0x0041_FE10.
        assert_eq!(regs[4], 0x0041_FE10);
        // Both values must have landed in memory at the offsets the shipped
        // code chose, relative to $s0.
        assert_eq!(mem.read_u32(0x0010_0000 + 0x0214).expect("read"), 0x0041_FE18);
        assert_eq!(mem.read_u32(0x0010_0000 + 0x04AC).expect("read"), 0x0041_FE10);
    }

    #[test]
    fn godhand_shipped_code_lifts_without_trapping() {
        let mut lifter = Lifter::new();
        for (i, &word) in GODHAND_REAL_CODE.iter().enumerate() {
            let insn = Insn::decode(word);
            assert_eq!(
                lifter.lift(&insn, 0x0010_0290 + (i as u32) * 4),
                LiftResult::Ok,
                "shipped instruction {i} trapped"
            );
        }
        assert_eq!(lifter.lifted, 6);
        assert_eq!(lifter.trapped, 0);
    }
}
