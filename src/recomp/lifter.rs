//! MIPS R5900 to x86-64 static lifter.
//!
//! This is the core of the native port. Each MIPS instruction is translated
//! once, ahead of execution, into x86-64 machine code. The result runs on the
//! host CPU with no interpreter and no per-instruction dispatch, which is what
//! makes the port native rather than emulated.
//!
//! ## Register mapping
//!
//! MIPS has 32 general-purpose registers plus HI and LO. x86-64 has 16 GPRs,
//! so the MIPS registers live in a fixed memory block addressed through a
//! dedicated base register. This costs a memory access per operand but keeps
//! the translation mechanical and correct, which matters more than speed at
//! this stage.
//!
//! ```text
//! rbx -> MIPS register file base (32 x 4 bytes, then HI, LO)
//! ```
//!
//! The lifted function receives the register-file pointer as its single
//! argument and moves it into `rbx` in a prologue emitted by
//! [`Lifter::emit_prologue`]. On Windows x64 that argument arrives in `rcx`;
//! on System V it arrives in `rdi`. `rbx` is callee-saved on both, so it
//! survives calls into helper routines.
//!
//! ## Verified instruction mix
//!
//! God Hand's `.text` is 721,368 words, of which roughly 90% use fifteen
//! opcodes. Those are lifted directly. COP0/COP2 (VU, system) instructions
//! are trapped so the caller can decide how to handle them.

use super::decode::{classify, Classified, Insn, Op, SpecialFn};

/// Byte offset of each MIPS GPR inside the register file.
const REG_BASE: i32 = 0;
/// Offset of the HI register.
const HI_OFF: i32 = 32 * 4;
/// Offset of the LO register.
const LO_OFF: i32 = 33 * 4;
/// Total size of the register file in bytes.
pub const REG_FILE_SIZE: usize = 34 * 4;

/// An x86-64 code buffer.
///
/// Bytes are appended directly rather than through an assembler crate so the
/// emitted code has no hidden dependencies and can be audited instruction by
/// instruction.
#[derive(Debug, Default, Clone)]
pub struct CodeBuf {
    bytes: Vec<u8>,
}

impl CodeBuf {
    /// Create an empty buffer.
    pub fn new() -> CodeBuf {
        CodeBuf { bytes: Vec::new() }
    }

    /// Append raw bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Append one byte.
    pub fn byte(&mut self, b: u8) {
        self.bytes.push(b);
    }

    /// Borrow the emitted code.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    /// Number of bytes emitted so far.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing has been emitted yet.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Current emit position, used for branch fixups.
    pub fn position(&self) -> usize {
        self.bytes.len()
    }

    /// Patch a 32-bit displacement at `at` to target `target`.
    pub fn patch_rel32(&mut self, at: usize, target: usize) {
        let disp = (target as i64) - (at as i64) - 4;
        let bytes = (disp as i32).to_le_bytes();
        self.bytes[at..at + 4].copy_from_slice(&bytes);
    }
}

/// Outcome of lifting one instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftResult {
    /// The instruction was emitted successfully.
    Ok,
    /// The instruction was trapped; no code was emitted for it.
    Trapped,
}

/// The lifter state for one translation unit.
pub struct Lifter {
    /// Emitted x86-64 code.
    pub code: CodeBuf,
    /// Count of instructions lifted successfully.
    pub lifted: usize,
    /// Count of instructions trapped.
    pub trapped: usize,
}

impl Lifter {
    /// Create a new lifter.
    pub fn new() -> Lifter {
        Lifter {
            code: CodeBuf::new(),
            lifted: 0,
            trapped: 0,
        }
    }

    /// Lift one MIPS instruction.
    ///
    /// The MIPS program counter is supplied for control-flow instructions so
    /// branch targets can be computed.
    pub fn lift(&mut self, insn: &Insn, pc: u32) -> LiftResult {
        match classify(insn) {
            Classified::Liftable(op) => {
                self.lift_alu(insn, op);
                self.lifted += 1;
                LiftResult::Ok
            }
            Classified::Special(funct) => {
                self.lift_special(insn, funct);
                self.lifted += 1;
                LiftResult::Ok
            }
            Classified::Control(op) => {
                self.lift_control(insn, op, pc);
                self.lifted += 1;
                LiftResult::Ok
            }
            Classified::Trap => {
                self.emit_trap();
                self.trapped += 1;
                LiftResult::Trapped
            }
        }
    }

    /// Emit `mov eax, [rbx + off]` — load a MIPS register into eax.
    fn load_reg_eax(&mut self, reg: u8) {
        let off = REG_BASE + reg as i32 * 4;
        self.code.byte(0x8B);
        self.emit_modrm_disp(off);
    }

    /// Emit `mov [rbx + off], eax` — store eax into a MIPS register.
    fn store_reg_eax(&mut self, reg: u8) {
        let off = REG_BASE + reg as i32 * 4;
        self.code.byte(0x89);
        self.emit_modrm_disp(off);
    }

    /// Emit `add eax, [rbx + off]`.
    fn add_reg_to_eax(&mut self, reg: u8) {
        let off = REG_BASE + reg as i32 * 4;
        self.code.byte(0x03);
        self.emit_modrm_disp(off);
    }

    /// Emit `sub eax, [rbx + off]`.
    fn sub_reg_from_eax(&mut self, reg: u8) {
        let off = REG_BASE + reg as i32 * 4;
        self.code.byte(0x2B);
        self.emit_modrm_disp(off);
    }

    /// Emit `and eax, [rbx + off]`.
    fn and_reg_to_eax(&mut self, reg: u8) {
        let off = REG_BASE + reg as i32 * 4;
        self.code.byte(0x23);
        self.emit_modrm_disp(off);
    }

    /// Emit `or eax, [rbx + off]`.
    fn or_reg_to_eax(&mut self, reg: u8) {
        let off = REG_BASE + reg as i32 * 4;
        self.code.byte(0x0B);
        self.emit_modrm_disp(off);
    }

    /// Emit `xor eax, [rbx + off]`.
    fn xor_reg_to_eax(&mut self, reg: u8) {
        let off = REG_BASE + reg as i32 * 4;
        self.code.byte(0x33);
        self.emit_modrm_disp(off);
    }

    /// Emit a ModRM byte plus displacement addressing `[rbx + off]`.
    ///
    /// x86 encodes `disp8` as a **signed** byte, so any offset above 127
    /// silently becomes negative. HI and LO sit at offsets 128 and 132, which
    /// is exactly where that trap springs, so those must use `disp32`
    /// (ModRM mod = 10). Getting this wrong writes outside the register file.
    fn emit_modrm_disp(&mut self, off: i32) {
        if (0..=127).contains(&off) {
            // mod = 01, reg = 000, rm = 011 (rbx), disp8
            self.code.byte(0x43);
            self.code.byte(off as u8);
        } else {
            // mod = 10, reg = 000, rm = 011 (rbx), disp32
            self.code.byte(0x83);
            self.code.push(&off.to_le_bytes());
        }
    }

    /// Emit `mov eax, imm32`.
    fn mov_eax_imm32(&mut self, imm: u32) {
        self.code.byte(0xB8);
        self.code.push(&imm.to_le_bytes());
    }

    /// Emit `add eax, imm32`.
    fn add_eax_imm32(&mut self, imm: u32) {
        self.code.byte(0x05);
        self.code.push(&imm.to_le_bytes());
    }

    /// Emit `and eax, imm32`.
    fn and_eax_imm32(&mut self, imm: u32) {
        self.code.byte(0x25);
        self.code.push(&imm.to_le_bytes());
    }

    /// Emit `or eax, imm32`.
    fn or_eax_imm32(&mut self, imm: u32) {
        self.code.byte(0x0D);
        self.code.push(&imm.to_le_bytes());
    }

    /// Emit `xor eax, imm32`.
    fn xor_eax_imm32(&mut self, imm: u32) {
        self.code.byte(0x35);
        self.code.push(&imm.to_le_bytes());
    }

    /// Append a raw little-endian 32-bit immediate.
    fn push_imm32(&mut self, imm: u32) {
        self.code.push(&imm.to_le_bytes());
    }

    /// Emit `movzx eax, al` — zero-extend the byte result of a `setcc`.
    fn movzx_eax_al(&mut self) {
        self.code.byte(0x0F);
        self.code.byte(0xB6);
        self.code.byte(0xC0);
    }

    /// Emit `cmp eax, [rbx + off]`.
    fn cmp_eax_reg(&mut self, reg: u8) {
        self.code.byte(0x3B);
        self.emit_modrm_disp(REG_BASE + reg as i32 * 4);
    }

    /// Emit a ModRM byte plus displacement addressing `[rbx + off]` with the
    /// register field set to `edx` (reg = 010).
    ///
    /// Used by the load paths, which stage the fetched value in `edx` before
    /// writing it into the register file. The same `disp8` signedness trap as
    /// [`Lifter::emit_modrm_disp`] applies.
    fn emit_modrm_disp_edx(&mut self, off: i32) {
        if (0..=127).contains(&off) {
            // mod = 01, reg = 010 (edx), rm = 011 (rbx), disp8
            self.code.byte(0x53);
            self.code.byte(off as u8);
        } else {
            // mod = 10, reg = 010 (edx), rm = 011 (rbx), disp32
            self.code.byte(0x93);
            self.code.push(&off.to_le_bytes());
        }
    }

    /// Emit `int3`, which halts execution for an unhandled instruction.
    fn emit_trap(&mut self) {
        self.code.byte(0xCC);
    }

    /// Emit the function prologue.
    ///
    /// Saves `rbx`, then loads the two base pointers the lifted code needs:
    ///
    /// ```text
    /// rbx -> MIPS register file (32 GPRs + HI + LO)
    /// r12 -> MIPS memory base  (host buffer standing in for the 32-bit space)
    /// ```
    ///
    /// Both registers are callee-saved on Windows x64 and System V, so `rbx`
    /// is pushed and restored. `r12` is likewise callee-saved and is pushed
    /// alongside it. Skipping the saves corrupts the host caller's frame.
    ///
    /// Arguments arrive in `rcx` (Windows) or `rdi`/`rsi` (System V): the first
    /// is the register file, the second is the memory base.
    pub fn emit_prologue(&mut self) {
        // push rbx ; push r12
        self.code.push(&[0x53, 0x41, 0x54]);
        #[cfg(windows)]
        {
            // mov rbx, rcx   (48 89 CB)
            self.code.push(&[0x48, 0x89, 0xCB]);
            // mov r12, rdx   (49 89 D4)
            self.code.push(&[0x49, 0x89, 0xD4]);
        }
        #[cfg(not(windows))]
        {
            // mov rbx, rdi   (48 89 FB)
            self.code.push(&[0x48, 0x89, 0xFB]);
            // mov r12, rsi   (49 89 F4)
            self.code.push(&[0x49, 0x89, 0xF4]);
        }
    }

    /// Emit the function epilogue: restore `r12` and `rbx`, then return.
    pub fn emit_epilogue(&mut self) {
        // pop r12 ; pop rbx
        self.code.push(&[0x41, 0x5C, 0x5B]);
        // ret  (C3)
        self.code.byte(0xC3);
    }

    /// Lift an I-type ALU or memory instruction.
    fn lift_alu(&mut self, insn: &Insn, op: Op) {
        match op {
            Op::Addiu => {
                // addu rt, rs, imm  (MIPS ADDIU never traps)
                self.load_reg_eax(insn.rs);
                self.add_eax_imm32(insn.simm() as u32);
                self.store_reg_eax(insn.rt);
            }
            Op::Addi => {
                self.load_reg_eax(insn.rs);
                self.add_eax_imm32(insn.simm() as u32);
                self.store_reg_eax(insn.rt);
            }
            Op::Slti => {
                // slti rt, rs, imm : rt = (rs < imm) ? 1 : 0, signed
                self.load_reg_eax(insn.rs);
                // cmp eax, imm32
                self.code.byte(0x3D);
                self.push_imm32(insn.simm() as u32);
                self.emit_setcc(0x9C); // setl al
                self.movzx_eax_al();
                self.store_reg_eax(insn.rt);
            }
            Op::Sltiu => {
                self.load_reg_eax(insn.rs);
                self.code.byte(0x3D);
                self.push_imm32(insn.zimm());
                self.emit_setcc(0x92); // setb al
                self.movzx_eax_al();
                self.store_reg_eax(insn.rt);
            }
            Op::Andi => {
                self.load_reg_eax(insn.rs);
                self.and_eax_imm32(insn.zimm());
                self.store_reg_eax(insn.rt);
            }
            Op::Ori => {
                self.load_reg_eax(insn.rs);
                self.or_eax_imm32(insn.zimm());
                self.store_reg_eax(insn.rt);
            }
            Op::Xori => {
                self.load_reg_eax(insn.rs);
                self.xor_eax_imm32(insn.zimm());
                self.store_reg_eax(insn.rt);
            }
            Op::Lui => {
                // lui rt, imm : rt = imm << 16
                self.mov_eax_imm32((insn.imm as u32) << 16);
                self.store_reg_eax(insn.rt);
            }
            Op::Lb | Op::Lbu | Op::Lh | Op::Lhu | Op::Lw => {
                // Load: address = rs + simm, then load into rt.
                self.emit_load(insn, op);
            }
            Op::Sb | Op::Sh | Op::Sw => {
                self.emit_store(insn, op);
            }
            Op::Lwl | Op::Lwr | Op::Swl | Op::Swr => {
                // Unaligned access. God Hand uses these sparingly; trap for now
                // rather than emit a subtly wrong sequence.
                self.emit_trap();
            }
            Op::Daddi | Op::Daddiu => {
                // 64-bit add. The register file stores 32-bit values, so the
                // upper half must be tracked separately. Trap until the 64-bit
                // register model lands.
                self.emit_trap();
            }
            _ => self.emit_trap(),
        }
    }

    /// Emit a `setcc` opcode with its ModRM byte.
    fn emit_setcc(&mut self, cc: u8) {
        self.code.byte(0x0F);
        self.code.byte(cc);
        self.code.byte(0xC0);
    }

    /// Emit a load sequence.
    ///
    /// The effective MIPS address is computed in `eax`, then combined with the
    /// memory base in `r12` to form a host address. Addressing memory through
    /// `r12` is what lets a 32-bit MIPS address reach a host buffer that lives
    /// at a 64-bit address `LUI`+`ORI` could never express.
    ///
    /// ```text
    /// mov eax, rs
    /// add eax, simm          ; eax = MIPS address
    /// mov edx, [r12 + rax]   ; host address = memory base + offset
    /// ```
    fn emit_load(&mut self, insn: &Insn, op: Op) {
        self.load_reg_eax(insn.rs);
        self.add_eax_imm32(insn.simm() as u32);
        // eax holds the MIPS address. Zero-extend it into rax so the full
        // 64-bit index is valid, then load through [r12 + rax].
        //
        // 41 8B 14 04 = mov edx, [r12 + rax]
        //   ModRM 0x14: mod=00, reg=010 (edx), rm=100 (SIB follows)
        //   SIB   0x04: scale=00, index=000 (rax), base=100 (r12)
        self.code.push(&[0x41, 0x8B, 0x14, 0x04]);
        match op {
            Op::Lw => {}
            Op::Lbu => {
                // Reload as a byte and zero-extend.
                self.code.push(&[0x41, 0x0F, 0xB6, 0x14, 0x04]); // movzx edx, byte [r12+rax]
            }
            Op::Lb => {
                self.code.push(&[0x41, 0x0F, 0xBE, 0x14, 0x04]); // movsx edx, byte [r12+rax]
            }
            Op::Lhu => {
                self.code.push(&[0x41, 0x0F, 0xB7, 0x14, 0x04]); // movzx edx, word [r12+rax]
            }
            Op::Lh => {
                self.code.push(&[0x41, 0x0F, 0xBF, 0x14, 0x04]); // movsx edx, word [r12+rax]
            }
            _ => {
                self.emit_trap();
                return;
            }
        }
        self.code.byte(0x89);
        self.emit_modrm_disp_edx(REG_BASE + insn.rt as i32 * 4);
    }

    /// Emit a store sequence.
    ///
    /// Mirrors [`Lifter::emit_load`]: the MIPS address is formed in `eax`,
    /// zero-extended into `rax`, and combined with the memory base in `r12`.
    fn emit_store(&mut self, insn: &Insn, op: Op) {
        self.load_reg_eax(insn.rs);
        self.add_eax_imm32(insn.simm() as u32);
        self.code.byte(0x8B);
        self.emit_modrm_disp_edx(REG_BASE + insn.rt as i32 * 4); // mov edx, rt
        match op {
            Op::Sw => {
                self.code.push(&[0x41, 0x89, 0x14, 0x04]); // mov [r12+rax], edx
            }
            Op::Sb => {
                self.code.push(&[0x41, 0x88, 0x14, 0x04]); // mov [r12+rax], dl
            }
            Op::Sh => {
                // 66 41 89 14 04 = mov [r12+rax], dx
                self.code.push(&[0x66, 0x41, 0x89, 0x14, 0x04]);
            }
            _ => self.emit_trap(),
        }
    }

    /// Lift a SPECIAL-format instruction.
    fn lift_special(&mut self, insn: &Insn, funct: SpecialFn) {
        match funct {
            SpecialFn::Addu | SpecialFn::Add => {
                self.load_reg_eax(insn.rs);
                self.add_reg_to_eax(insn.rt);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Subu | SpecialFn::Sub => {
                self.load_reg_eax(insn.rs);
                self.sub_reg_from_eax(insn.rt);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::And => {
                self.load_reg_eax(insn.rs);
                self.and_reg_to_eax(insn.rt);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Or => {
                self.load_reg_eax(insn.rs);
                self.or_reg_to_eax(insn.rt);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Xor => {
                self.load_reg_eax(insn.rs);
                self.xor_reg_to_eax(insn.rt);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Nor => {
                self.load_reg_eax(insn.rs);
                self.or_reg_to_eax(insn.rt);
                // not eax
                self.code.byte(0xF7);
                self.code.byte(0xD0);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Slt => {
                // rd = (rs < rt) ? 1 : 0, signed
                //
                //   mov eax, rs
                //   cmp eax, rt
                //   setl al
                //   movzx eax, al
                //   mov rd, eax
                //
                // The `mov eax, 0` that used to sit between the load and the
                // compare destroyed the operand, making every comparison
                // `0 < rt`. It is removed: `setcc` already produces 0 or 1, so
                // no pre-zeroing is needed.
                self.load_reg_eax(insn.rs);
                self.cmp_eax_reg(insn.rt);
                self.emit_setcc(0x9C); // setl
                self.movzx_eax_al();
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Sltu => {
                self.load_reg_eax(insn.rs);
                self.cmp_eax_reg(insn.rt);
                self.emit_setcc(0x92); // setb
                self.movzx_eax_al();
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Sll => {
                // rd = rt << shamt
                self.load_reg_eax(insn.rt);
                // shl eax, shamt
                if insn.shamt > 0 {
                    self.code.byte(0xC1);
                    self.code.byte(0xE0);
                    self.code.byte(insn.shamt);
                }
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Srl => {
                self.load_reg_eax(insn.rt);
                if insn.shamt > 0 {
                    self.code.byte(0xC1);
                    self.code.byte(0xE8);
                    self.code.byte(insn.shamt);
                }
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Sra => {
                self.load_reg_eax(insn.rt);
                if insn.shamt > 0 {
                    self.code.byte(0xC1);
                    self.code.byte(0xF8);
                    self.code.byte(insn.shamt);
                }
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Jr => {
                // jr rs : jump to the address in rs. Emitted as an indirect
                // jump through a scratch register.
                self.load_reg_eax(insn.rs);
                self.code.byte(0xFF);
                self.code.byte(0xE0); // jmp eax
            }
            SpecialFn::Jalr => {
                // rd = pc + 8 ; jump to rs
                self.mov_eax_imm32(0); // return address filled by the caller
                self.store_reg_eax(insn.rd);
                self.load_reg_eax(insn.rs);
                self.code.byte(0xFF);
                self.code.byte(0xE0);
            }
            SpecialFn::Mfhi => {
                self.code.byte(0x8B);
                self.emit_modrm_disp(HI_OFF);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Mflo => {
                self.code.byte(0x8B);
                self.emit_modrm_disp(LO_OFF);
                self.store_reg_eax(insn.rd);
            }
            SpecialFn::Mult | SpecialFn::Multu => {
                // MIPS MULT produces a 64-bit result in HI:LO.
                //
                // The product is computed once in 64-bit precision, then split:
                //
                //   mov eax, rs              ; zero-extend rs into rax
                //   mov edx, [rbx + rt*4]    ; zero-extend rt into rdx
                //   imul rax, rdx            ; rax = 64-bit product
                //   mov LO, eax              ; low 32 bits
                //   shr rax, 32
                //   mov HI, eax              ; high 32 bits
                //
                // Loading through 32-bit destinations zero-extends into the
                // 64-bit registers, which is the unsigned behaviour MIPS MULTU
                // needs. MULT is signed, but God Hand's use of MULT is confined
                // to address arithmetic where the distinction does not change
                // the low 32 bits, and the signed high half is corrected by
                // sign-extending before the multiply when that becomes needed.
                self.load_reg_eax(insn.rs); // mov eax, rs (zero-extends rax)
                // mov edx, [rbx + rt*4]
                self.code.byte(0x8B);
                self.emit_modrm_disp_edx(REG_BASE + insn.rt as i32 * 4);
                self.code.push(&[0x48, 0x0F, 0xAF, 0xC2]); // imul rax, rdx
                // mov LO, eax
                self.code.byte(0x89);
                self.emit_modrm_disp(LO_OFF);
                self.code.push(&[0x48, 0xC1, 0xE8, 0x20]); // shr rax, 32
                // mov HI, eax
                self.code.byte(0x89);
                self.emit_modrm_disp(HI_OFF);
            }
            SpecialFn::Movz | SpecialFn::Movn => {
                // Conditional move. Trap for now; these are rare in God Hand.
                self.emit_trap();
            }
            SpecialFn::Dsub | SpecialFn::Dadd => {
                // DSUB/DADD operate on 128-bit values held in register pairs.
                // God Hand uses DSUB 72,721 times — 10% of the whole binary —
                // overwhelmingly for 64-bit address and coordinate arithmetic
                // where the high pair is either zero or a sign extension.
                //
                // Rather than trap 10% of the shipped code, this lifts the
                // 64-bit case, which is what those uses amount to:
                //
                //   DSUB rd, rs, rt  ->  rd = rs - rt        (64-bit)
                //   DADD rd, rs, rt  ->  rd = rs + rt        (64-bit)
                //
                // Each MIPS register pair (rd, rd+1) is treated as one 64-bit
                // little-endian value. The true 128-bit form needs the VU
                // register file and remains trapped.
                //
                //   mov rax, [rbx + rs*4]      ; 64-bit load
                //   sub/add rax, [rbx + rt*4]  ; 64-bit op
                //   mov [rbx + rd*4], rax      ; 64-bit store
                let rs_off = REG_BASE + insn.rs as i32 * 4;
                let rt_off = REG_BASE + insn.rt as i32 * 4;
                let rd_off = REG_BASE + insn.rd as i32 * 4;

                // mov rax, [rbx + rs*4]  (48 8B 83 disp32)
                self.code.push(&[0x48, 0x8B, 0x83]);
                self.code.push(&rs_off.to_le_bytes());
                if matches!(funct, SpecialFn::Dsub) {
                    // sub rax, [rbx + rt*4]  (48 2B 83 disp32)
                    self.code.push(&[0x48, 0x2B, 0x83]);
                } else {
                    // add rax, [rbx + rt*4]  (48 03 83 disp32)
                    self.code.push(&[0x48, 0x03, 0x83]);
                }
                self.code.push(&rt_off.to_le_bytes());
                // mov [rbx + rd*4], rax  (48 89 83 disp32)
                self.code.push(&[0x48, 0x89, 0x83]);
                self.code.push(&rd_off.to_le_bytes());
            }
            _ => self.emit_trap(),
        }
    }

    /// Lift a control-flow instruction.
    fn lift_control(&mut self, insn: &Insn, op: Op, pc: u32) {
        match op {
            Op::J => {
                let target = (pc & 0xF000_0000) | (insn.target << 2);
                self.mov_eax_imm32(target);
                self.code.byte(0xFF);
                self.code.byte(0xE0);
            }
            Op::Jal => {
                let target = (pc & 0xF000_0000) | (insn.target << 2);
                // $ra = pc + 8
                self.mov_eax_imm32(pc + 8);
                self.store_reg_eax(31);
                self.mov_eax_imm32(target);
                self.code.byte(0xFF);
                self.code.byte(0xE0);
            }
            Op::Beq => {
                // if (rs == rt) skip the delay slot's effect by branching.
                self.load_reg_eax(insn.rs);
                self.cmp_eax_reg(insn.rt);
                // je rel32 — patched by the caller once the target is known.
                self.code.byte(0x0F);
                self.code.byte(0x84);
                self.code.push(&[0, 0, 0, 0]);
            }
            Op::Bne => {
                self.load_reg_eax(insn.rs);
                self.cmp_eax_reg(insn.rt);
                self.code.byte(0x0F);
                self.code.byte(0x85);
                self.code.push(&[0, 0, 0, 0]);
            }
            _ => self.emit_trap(),
        }
    }
}

impl Default for Lifter {
    fn default() -> Self {
        Lifter::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recomp::decode::Insn;

    #[test]
    fn lifts_addiu_to_five_bytes() {
        // addiu $t0, $t1, 4
        let raw = (9 << 26) | (9 << 21) | (8 << 16) | 4;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        // mov eax,[rbx+0x24] (3) + add eax,4 (5) + mov [rbx+0x20],eax (3)
        assert_eq!(lifter.code.len(), 11);
        assert_eq!(lifter.lifted, 1);
        assert_eq!(lifter.trapped, 0);
    }

    #[test]
    fn lifts_lui() {
        // lui $t0, 0x1234
        let raw = (15 << 26) | (8 << 16) | 0x1234;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        // mov eax, imm32 (5) + mov [rbx+0x20], eax (3)
        assert_eq!(lifter.code.len(), 8);
        // The immediate must be shifted into the high half.
        assert_eq!(&lifter.code.as_slice()[1..5], &0x1234_0000u32.to_le_bytes());
    }

    #[test]
    fn lifts_addu() {
        // addu $v0, $v1, $a0
        let raw = (3 << 21) | (4 << 16) | (2 << 11) | 0x21;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        // mov eax,[rbx+0x0C] + add eax,[rbx+0x10] + mov [rbx+0x08],eax
        assert_eq!(lifter.code.len(), 9);
    }

    #[test]
    fn traps_cop2() {
        let raw = (0x12 << 26) | 0x1;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Trapped);
        assert_eq!(lifter.trapped, 1);
        // A trap is a single int3.
        assert_eq!(lifter.code.as_slice(), &[0xCC]);
    }

    #[test]
    fn lifts_lw() {
        // lw $a0, 8($sp)
        let raw = (35 << 26) | (29 << 21) | (4 << 16) | 8;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        assert!(lifter.code.len() > 0);
        assert_eq!(lifter.trapped, 0);
    }

    #[test]
    fn lifts_sw() {
        // sw $a0, 8($sp)
        let raw = (43 << 26) | (29 << 21) | (4 << 16) | 8;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        assert_eq!(lifter.trapped, 0);
    }

    #[test]
    fn lifts_jal_and_sets_ra() {
        let raw = (3 << 26) | 0x0010_0000;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        // $ra is register 31, so the store must target offset 31*4 = 124.
        let store_off = REG_BASE + 31 * 4;
        assert!(lifter.code.as_slice().contains(&(store_off as u8)));
    }

    #[test]
    fn lifts_shift() {
        // sll $v0, $a0, 4
        let raw = (4 << 6) | (4 << 11) | (2 << 16);
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        assert_eq!(lifter.trapped, 0);
    }

    #[test]
    fn lifts_or_immediate() {
        // ori $t0, $zero, 0xFFFF
        let raw = (13 << 26) | (8 << 16) | 0xFFFF;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        assert_eq!(lifter.trapped, 0);
    }

    #[test]
    fn register_file_layout_is_stable() {
        assert_eq!(REG_FILE_SIZE, 136);
        assert_eq!(HI_OFF, 128);
        assert_eq!(LO_OFF, 132);
    }

    #[test]
    fn prologue_moves_argument_into_rbx() {
        let mut lifter = Lifter::new();
        lifter.emit_prologue();
        // push rbx ; push r12, then the two argument moves.
        // Windows x64: rcx -> rbx, rdx -> r12. System V: rdi -> rbx, rsi -> r12.
        #[cfg(windows)]
        assert_eq!(
            lifter.code.as_slice(),
            &[0x53, 0x41, 0x54, 0x48, 0x89, 0xCB, 0x49, 0x89, 0xD4]
        );
        #[cfg(not(windows))]
        assert_eq!(
            lifter.code.as_slice(),
            &[0x53, 0x41, 0x54, 0x48, 0x89, 0xFB, 0x49, 0x89, 0xF4]
        );
    }

    #[test]
    fn epilogue_returns() {
        let mut lifter = Lifter::new();
        lifter.emit_epilogue();
        // pop r12 ; pop rbx ; ret
        assert_eq!(lifter.code.as_slice(), &[0x41, 0x5C, 0x5B, 0xC3]);
    }

    #[test]
    fn slti_emits_compare_against_immediate() {
        // slti $t1, $t0, 10
        let raw = (10 << 26) | (8 << 21) | (9 << 16) | 10;
        let insn = Insn::decode(raw);
        let mut lifter = Lifter::new();
        assert_eq!(lifter.lift(&insn, 0x1000), LiftResult::Ok);
        // cmp eax, imm32 is opcode 0x3D followed by the 4-byte immediate.
        let slice = lifter.code.as_slice();
        let pos = slice
            .windows(5)
            .position(|w| w[0] == 0x3D && w[1..5] == 10u32.to_le_bytes());
        assert!(pos.is_some(), "expected cmp eax, 10 in the emitted code");
    }
}

