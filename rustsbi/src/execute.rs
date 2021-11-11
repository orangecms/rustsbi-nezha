use crate::feature;
use crate::runtime::{MachineTrap, Runtime, SupervisorContext};
use core::{
    ops::{Generator, GeneratorState},
    pin::Pin,
};
use riscv::register::scause::{Exception, Trap};

pub fn execute_supervisor(supervisor_mepc: usize, a0: usize, a1: usize) -> ! {
    let mut rt = Runtime::new_sbi_supervisor(supervisor_mepc, a0, a1);
    loop {
        match Pin::new(&mut rt).resume(()) {
            GeneratorState::Yielded(MachineTrap::SbiCall()) => {
                let ctx = rt.context_mut();
                if emulate_sbi_call(ctx) {
                    continue;
                }
                feature::preprocess_supervisor_external(ctx); // specific for 1.9.1; see document for details
                let param = [ctx.a0, ctx.a1, ctx.a2, ctx.a3, ctx.a4];
                let ans = rustsbi::ecall(ctx.a7, ctx.a6, param);
                ctx.a0 = ans.error;
                ctx.a1 = ans.value;
                ctx.mepc = ctx.mepc.wrapping_add(4);
            }
            GeneratorState::Yielded(MachineTrap::IllegalInstruction()) => {
                let ctx = rt.context_mut();
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
                feature::call_supervisor_interrupt(ctx)
            },
            GeneratorState::Yielded(MachineTrap::MachineTimer()) => {
                feature::forward_supervisor_timer()
            }
            GeneratorState::Yielded(MachineTrap::MachineSoft()) => {
                feature::forward_supervisor_soft()
            }
            GeneratorState::Yielded(MachineTrap::InstructionFault(_addr)) => {
                let ctx = rt.context_mut();
                unsafe {
                    feature::do_transfer_trap(ctx, Trap::Exception(Exception::InstructionFault))
                }
            }
            GeneratorState::Yielded(MachineTrap::InstructionPageFault(_addr)) => {
                let ctx = rt.context_mut();
                unsafe {
                    feature::do_transfer_trap(ctx, Trap::Exception(Exception::InstructionPageFault))
                }
            }
            GeneratorState::Yielded(MachineTrap::LoadFault(_addr)) => {
                let ctx = rt.context_mut();
                unsafe { feature::do_transfer_trap(ctx, Trap::Exception(Exception::LoadFault)) }
            }
            GeneratorState::Yielded(MachineTrap::LoadPageFault(_addr)) => {
                let ctx = rt.context_mut();
                unsafe { feature::do_transfer_trap(ctx, Trap::Exception(Exception::LoadPageFault)) }
            }
            GeneratorState::Yielded(MachineTrap::StorePageFault(_addr)) => {
                let ctx = rt.context_mut();
                unsafe {
                    feature::do_transfer_trap(ctx, Trap::Exception(Exception::StorePageFault))
                }
            }
            GeneratorState::Yielded(MachineTrap::StoreFault(_addr)) => {
                let ctx = rt.context_mut();
                unsafe { feature::do_transfer_trap(ctx, Trap::Exception(Exception::StoreFault)) }
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
    false
}

// 真·非法指令异常，是M层出现的
fn fail_illegal_instruction(ctx: &mut SupervisorContext, ins: usize) -> ! {
    panic!("invalid instruction from machine level, mepc: {:016x?}, instruction: {:016x?}, context: {:016x?}", ctx.mepc, ins, ctx);
}
