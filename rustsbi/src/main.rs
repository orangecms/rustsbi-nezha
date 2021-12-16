#![no_std]
#![no_main]
#![feature(asm)]
#![feature(asm_sym)]
#![feature(asm_const)]
#![feature(generator_trait)]
#![feature(naked_functions)]
#![feature(default_alloc_error_handler)]

mod execute;
mod feature;
mod hal;
mod hart_csr_utils;
mod peripheral;
mod runtime;

extern crate alloc;
extern crate bitflags;

use crate::{hal::write_reg, hart_csr_utils::print_hart_pmp};
use buddy_system_allocator::LockedHeap;
use core::panic::PanicInfo;
use riscv::register::{fcsr, mstatus};
use riscv::register::{medeleg, mideleg, mie};
use rustsbi::println;

const PER_HART_STACK_SIZE: usize = 8 * 1024; // 8KiB
const SBI_STACK_SIZE: usize = 2 * PER_HART_STACK_SIZE;
#[link_section = ".bss.uninit"]
static mut SBI_STACK: [u8; SBI_STACK_SIZE] = [0; SBI_STACK_SIZE];

const PAYLOAD_OFFSET: usize = 0x4020_0000;
const DTB_OFFSET: usize = 0x4120_0000;
const SBI_HEAP_SIZE: usize = 8 * 1024; // 8KiB

#[link_section = ".bss.uninit"]
static mut HEAP_SPACE: [u8; SBI_HEAP_SIZE] = [0; SBI_HEAP_SIZE];
static PLATFORM: &str = "T-HEAD Xuantie Platform";
#[global_allocator]
static SBI_HEAP: LockedHeap<32> = LockedHeap::empty();

extern "C" fn rust_main() -> ! {
    let hartid = riscv::register::mhartid::read();
    if hartid == 0 {
        init_bss();
    }
    init_pmp();
    runtime::init();
    if hartid == 0 {
        init_heap();
        unsafe {
            init_plic();
            // init_mstatus();
        }
        peripheral::init_peripheral();
        println!("[rustsbi] RustSBI version {}\r", rustsbi::VERSION);
        println!("{}", rustsbi::LOGO);
        println!("[rustsbi] Platform Name: {}\r", PLATFORM);
        println!(
            "[rustsbi] Implementation: RustSBI-NeZha Version {}\r",
            env!("CARGO_PKG_VERSION")
        );
    }
    unsafe {
        delegate_interrupt_exception();
    }
    if hartid == 0 {
        hart_csr_utils::print_hart_csrs();
        println!("[rustsbi] enter supervisor 0x{:x}\r", PAYLOAD_OFFSET);
        println!("[rustsbi] dtb handed over from 0x{:x}\r", DTB_OFFSET);
        print_hart_pmp();
    }
    execute::execute_supervisor(PAYLOAD_OFFSET, hartid, DTB_OFFSET)
}

fn init_bss() {
    extern "C" {
        static mut ebss: u32;
        static mut sbss: u32;
        static mut edata: u32;
        static mut sdata: u32;
        static sidata: u32;
    }
    unsafe {
        r0::zero_bss(&mut sbss, &mut ebss);
        r0::init_data(&mut sdata, &mut edata, &sidata);
    }
}

/**
 * from OpenSBI:
 * PMP0    : 0x0000000040000000-0x000000004001ffff (A)
 * PMP1    : 0x0000000040000000-0x000000007fffffff (A,R,W,X)
 * PMP2    : 0x0000000000000000-0x0000000007ffffff (A,R,W)
 * PMP3    : 0x0000000009000000-0x000000000901ffff (
 */
fn init_pmp() {
    use riscv::register::*;
    let cfg = 0x0f0f0f0f0fusize;
    pmpcfg0::write(cfg);
    // pmpcfg2::write(0);
    pmpaddr0::write(0x40000000usize >> 2);
    pmpaddr1::write(0x40200000usize >> 2);
    pmpaddr2::write(0x80000000usize >> 2);
    pmpaddr3::write(0xc0000000usize >> 2);
    pmpaddr4::write(0xffffffffusize >> 2);
}

unsafe fn init_plic() {
    let mut addr: usize;
    asm!("csrr {}, 0xfc1", out(reg) addr);
    write_reg(addr, 0x001ffffc, 0x1)
}

unsafe fn init_mstatus() {
    mstatus::set_mxr();
    mstatus::set_sum();
    mstatus::clear_tvm();
    mstatus::clear_tsr();
    mstatus::clear_tw();
    mstatus::set_fs(mstatus::FS::Dirty);
    fcsr::set_rounding_mode(fcsr::RoundingMode::RoundToNearestEven);
}

/*
 * From stock Nezha OpenSBI:
 *
 * MIDELEG : 0x0000000000000222
 * MEDELEG : 0x000000000000b1ff
 *
 * QEMU OpenSBI 0.9:
 *
 * Boot HART MIDELEG         : 0x0000000000000222
 * Boot HART MEDELEG         : 0x000000000000b109
 */
// see riscv-privileged spec v1.10
/*
 * The TW (Timeout Wait) bit supports intercepting the WFI instruction (see
 * Section 3.2.3). When TW=0, the WFI instruction is permitted in S-mode.
 * When TW=1, if WFI is executed in S- mode, and it does not complete within
 * an implementation-specific, bounded time limit, the WFI instruction causes
 * an illegal instruction trap. The time limit may always be 0, in which case
 * WFI always causes an illegal instruction trap in S-mode when TW=1.
 * TW is hard-wired to 0 when S-mode is not supported.
 */
unsafe fn delegate_interrupt_exception() {
    mideleg::set_sext();
    mideleg::set_stimer();
    mideleg::set_ssoft();
    // p 35, table 3.6
    medeleg::set_instruction_misaligned();
    medeleg::set_instruction_fault();
    // This currently causes Linux to panic. We need to handle WFI in SBI.
    // medeleg::set_illegal_instruction();
    medeleg::set_breakpoint();
    medeleg::set_load_misaligned(); // TODO: handle this?
    medeleg::set_load_fault(); // PMP violation, shouldn't be hit
    medeleg::set_store_misaligned();
    medeleg::set_store_fault();
    medeleg::set_user_env_call();
    // Do not delegate env call from S-mode nor M-mode
    medeleg::set_instruction_page_fault();
    medeleg::set_load_page_fault();
    medeleg::set_store_page_fault();
    mie::set_msoft();
}

fn init_heap() {
    unsafe {
        SBI_HEAP
            .lock()
            .init(HEAP_SPACE.as_ptr() as usize, SBI_HEAP_SIZE)
    }
}

#[cfg_attr(not(test), panic_handler)]
#[allow(unused)]
fn panic(info: &PanicInfo) -> ! {
    let hart_id = riscv::register::mhartid::read();
    // 输出的信息大概是“[rustsbi-panic] hart 0 panicked at ...”
    println!("[rustsbi-panic] hart {} {}", hart_id, info);
    println!("[rustsbi-panic] system shutdown scheduled due to RustSBI panic");
    use rustsbi::Reset;
    peripheral::Reset.system_reset(
        rustsbi::reset::RESET_TYPE_SHUTDOWN,
        rustsbi::reset::RESET_REASON_SYSTEM_FAILURE,
    );
    loop {}
}

#[naked]
#[link_section = ".text.entry"]
#[export_name = "_start"]
unsafe extern "C" fn entry() -> ! {
    asm!(
    // 1. set sp
    // sp = bootstack + (hartid + 1) * HART_STACK_SIZE
    "
    la      sp, {stack}
    li      t0, {per_hart_stack_size}
    csrr    a0, mhartid
    addi    t1, a0, 1
1:  add     sp, sp, t0
    addi    t1, t1, -1
    bnez    t1, 1b
    ",
    // 2. jump to rust_main (absolute address)
    "j      {rust_main}",
    per_hart_stack_size = const PER_HART_STACK_SIZE,
    stack = sym SBI_STACK,
    rust_main = sym rust_main,
    options(noreturn))
}
