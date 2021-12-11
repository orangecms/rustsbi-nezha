use crate::feature;
use crate::{
    hal::read_reg,
    runtime::{MachineTrap, Runtime, SupervisorContext},
};
use core::{
    ops::{Generator, GeneratorState},
    pin::Pin,
};
use riscv::register::scause::{Exception, Trap};
use rustsbi::println;

pub fn execute_supervisor(supervisor_mepc: usize, a0: usize, a1: usize) -> ! {
    let mut rt = Runtime::new_sbi_supervisor(supervisor_mepc, a0, a1);
    loop {
        match Pin::new(&mut rt).resume(()) {
            GeneratorState::Yielded(MachineTrap::SbiCall()) => {
                let ctx = rt.context_mut();
                if ctx.a7 != 0x1 {
                    println!("[rustsbi] Who ya gonna call?\r");
                }
                if emulate_sbi_call(ctx) {
                    continue;
                }
                // specific for 1.9.1; see document for details
                feature::preprocess_supervisor_external(ctx);
                if ctx.a7 != 0x1 {
                    println!("[rustsbi] {:x?} {:x?}\r", ctx.a7, ctx.a6);
                    println!("{:#04X?}\r", [ctx.a0, ctx.a1, ctx.a2]);
                }
                let param = [ctx.a0, ctx.a1, ctx.a2, ctx.a3, ctx.a4, ctx.a5];
                let ans = rustsbi::ecall(ctx.a7, ctx.a6, param);
                ctx.a0 = ans.error;
                ctx.a1 = ans.value;
                ctx.mepc = ctx.mepc.wrapping_add(4);
                if ctx.a7 != 0x1 {
                    println!("[rustsbi] {:x?} {:x?}\r", ctx.a0, ctx.a1);
                }
            }
            GeneratorState::Yielded(MachineTrap::IllegalInstruction()) => {
                let ctx = rt.context_mut();
                println!("[rustsbi] Na na na! {:x?} {:x?}\r", ctx.a0, ctx.a1);
                // FIXME: get_vaddr_u32这个过程可能出错。
                let ins = unsafe { get_vaddr_u32(ctx.mepc) } as usize;
                if !emulate_illegal_instruction(ctx, ins) {
                    unsafe {
                        if feature::should_transfer_trap(ctx) {
                            feature::do_transfer_trap(
                                ctx,
                                Trap::Exception(Exception::IllegalInstruction),
                            )
                        } else {
                            fail_illegal_instruction(ctx, ins)
                        }
                    }
                }
            }
            GeneratorState::Yielded(MachineTrap::ExternalInterrupt()) => unsafe {
                let ctx = rt.context_mut();
                println!("[rustsbi] No no no! {:x?} {:x?}\r", ctx.a0, ctx.a1);
                feature::call_supervisor_interrupt(ctx)
            },
            GeneratorState::Yielded(MachineTrap::MachineTimer()) => {
                feature::forward_supervisor_timer()
            }
            GeneratorState::Yielded(MachineTrap::MachineSoft()) => {
                feature::forward_supervisor_soft()
            }
            // todo：编写样例，验证store page fault和instruction page fault
            GeneratorState::Yielded(MachineTrap::InstructionFault(addr)) => {
                let ctx = rt.context_mut();
                println!("[rustsbi] Na na na! {:x?} {:x?}\r", ctx.a0, ctx.a1);
                if feature::is_page_fault(addr) {
                    unsafe {
                        feature::do_transfer_trap(
                            ctx,
                            Trap::Exception(Exception::InstructionPageFault),
                        )
                    }
                } else {
                    unsafe {
                        feature::do_transfer_trap(ctx, Trap::Exception(Exception::InstructionFault))
                    }
                }
            }
            GeneratorState::Yielded(MachineTrap::LoadFault(_addr)) => {
                let ctx = rt.context_mut();
                println!("[rustsbi] Na na na! {:x?} {:x?}\r", ctx.a0, ctx.a1);
                unsafe { feature::do_transfer_trap(ctx, Trap::Exception(Exception::LoadFault)) }
            }
            GeneratorState::Yielded(MachineTrap::LoadPageFault(_addr)) => {
                let ctx = rt.context_mut();
                println!("[rustsbi] LoadPageFault {:#04X?}\r", ctx);
                unsafe { feature::do_transfer_trap(ctx, Trap::Exception(Exception::LoadPageFault)) }
            }
            GeneratorState::Yielded(MachineTrap::StorePageFault(addr)) => {
                let ctx = rt.context_mut();
                println!("[rustsbi] StorePageFault {:#04X?}\r", ctx);
                if feature::is_page_fault(addr) {
                    unsafe {
                        feature::do_transfer_trap(ctx, Trap::Exception(Exception::LoadPageFault))
                    }
                } else {
                    unsafe { feature::do_transfer_trap(ctx, Trap::Exception(Exception::LoadFault)) }
                }
            }
            GeneratorState::Yielded(MachineTrap::StoreFault(addr)) => {
                let ctx = rt.context_mut();
                println!("[rustsbi] No no no! {:x?} {:x?}\r", ctx.a0, ctx.a1);
                if feature::is_page_fault(addr) {
                    unsafe {
                        feature::do_transfer_trap(ctx, Trap::Exception(Exception::StorePageFault))
                    }
                } else {
                    unsafe {
                        feature::do_transfer_trap(ctx, Trap::Exception(Exception::StoreFault))
                    }
                }
            }
            GeneratorState::Yielded(MachineTrap::InstructionPageFault(addr)) => {
                let ctx = rt.context_mut();
                println!("\r\n[rustsbi] {:?}", Exception::InstructionPageFault);
                println!(
                    "[rustsbi] addr: [0x{:x}] mepc: [0x{:x}] 0x{:x}",
                    addr,
                    ctx.mepc,
                    unsafe { read_reg::<usize>(addr, 0) }
                );
                let mut a0: u32;
                let mut a1: u32;
                let mut a2: u32;
                let mut t0: u32;
                let mut t1: u32;
                let mut t2: u32;
                unsafe {
                    asm!("
                    mv  {0}, a0
                    mv  {1}, a1
                    mv  {2}, a2
                    mv  {3}, t0
                    mv  {4}, t1
                    mv  {5}, t2
                    ", out(reg) a0, out(reg) a1, out(reg) a2, out(reg) t0, out(reg) t1, out(reg) t2);
                }
                println!("[rustsbi] a0: 0x{:x}, a1: 0x{:x}, a2: 0x{:x}", a0, a1, a2);
                println!("[rustsbi] t0: 0x{:x}, t1: 0x{:x}, t2: 0x{:x}", t0, t1, t2);
                unsafe { asm!("wfi") }
            }
            GeneratorState::Complete(()) => unreachable!(),
        }
    }
}

#[inline]
unsafe fn get_vaddr_u32(vaddr: usize) -> u32 {
    get_vaddr_u16(vaddr) as u32 | ((get_vaddr_u16(vaddr.wrapping_add(2)) as u32) << 16)
}

#[inline]
#[warn(asm_sub_register)]
unsafe fn get_vaddr_u16(vaddr: usize) -> u16 {
    let mut ans: u16;
    asm!("
        li      {2}, (1 << 17)
        csrrs   {2}, mstatus, {2}
        lhu     {0}, 0({1})
        csrw    mstatus, {2}
    ", out(reg) ans, in(reg) vaddr, out(reg) _);
    ans
}

fn emulate_sbi_call(ctx: &mut SupervisorContext) -> bool {
    if feature::emulate_sbi_rustsbi_nezha_sext(ctx) {
        return true;
    }
    false
}

fn emulate_illegal_instruction(ctx: &mut SupervisorContext, ins: usize) -> bool {
    if feature::emulate_rdtime(ctx, ins) {
        return true;
    }
    if feature::emulate_sfence_vma(ctx, ins) {
        return true;
    }
    false
}

// 真·非法指令异常，是M层出现的
fn fail_illegal_instruction(ctx: &mut SupervisorContext, ins: usize) -> ! {
    panic!("invalid instruction from machine level, mepc: {:016x?}, instruction: {:016x?}, context: {:016x?}", ctx.mepc, ins, ctx);
}
