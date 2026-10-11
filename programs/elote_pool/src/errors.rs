//! Error codes; Anchor numbers them from 6000 in this order.

use anchor_lang::prelude::*;

#[error_code]
pub enum PoolError {
    #[msg("The pool is paused")]
    Paused,
    #[msg("Only the pool admin may do this")]
    Unauthorized,
    #[msg("Amount is zero or exceeds what is due")]
    WrongAmount,
    #[msg("Not enough liquidity in the vault")]
    InsufficientLiquidity,
    #[msg("The line is not in the required status")]
    LineStatus,
    #[msg("The term has not ended")]
    TermNotEnded,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("The sponsor named on this line must sign")]
    SponsorRequired,
    #[msg("The deposit would mint zero shares")]
    ZeroShares,
    #[msg("First deposit below the minimum")]
    FirstDepositTooSmall,
    #[msg("Wrong mint or token account")]
    WrongAccount,
    #[msg("The result is below the minimum the caller accepts")]
    SlippageExceeded,
    #[msg("The term has ended")]
    TermEnded,
    #[msg("The installment schedule is not valid")]
    InvalidSchedule,
    #[msg("The last installment would fall due after the term")]
    ScheduleEndsAfterTerm,
    #[msg("The line has no installment schedule")]
    NotScheduled,
    #[msg("Fewer installments missed than the line allows before collection")]
    NotEnoughMissed,
    #[msg("The repayment must cover the late interest due")]
    LateInterestUnpaid,
    #[msg("A line in collection is written off only after its oldest missed due date plus grace, or after the term plus grace")]
    WriteOffTooEarly,
}
