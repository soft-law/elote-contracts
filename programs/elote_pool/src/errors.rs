//! Error codes; Anchor numbers them from 6000 in this order.

use anchor_lang::prelude::*;

#[error_code]
pub enum PoolError {
    // Signers.
    #[msg("This signer may not do this")]
    Unauthorized,
    #[msg("The sponsor named on this line must sign")]
    SponsorRequired,
    #[msg("The pagaré's holder must sign")]
    NotHolder,
    #[msg("The pool is paused")]
    Paused,

    // The council.
    #[msg("The council is not the vault of an autonomous Squads v4 multisig with a majority threshold")]
    CouncilInvalid,
    #[msg("The council multisig has more members than the program checks")]
    CouncilTooLarge,
    #[msg("This council kind is not accepted")]
    CouncilKindInvalid,
    #[msg("No council rotation is pending, or this signer is not the one proposed")]
    NoPendingCouncil,
    #[msg("The council's decision is past its valid_until")]
    ProposalExpired,

    // Accounts.
    #[msg("Wrong mint or token account")]
    WrongAccount,
    #[msg("The asset mint carries a Token-2022 extension the pool does not accept")]
    AssetMintNotAllowed,
    #[msg("The share mint is not one this pool can own and price: wrong authority, supply, decimals or extensions")]
    ShareMintNotAllowed,
    #[msg("The asset mint is not on this cluster's allow-list")]
    AssetNotListed,
    #[msg("The asset mint and the share mint are the same")]
    AssetIsShareMint,
    #[msg("The collection is not a bare Metaplex Core collection whose update authority is the pool")]
    CollectionNotAllowed,
    #[msg("The pagaré records passed are not the ones this payment touches, oldest first")]
    PagareAccountsMismatch,

    // Roles that must stay apart.
    #[msg("The council cannot be the borrower")]
    BorrowerIsCouncil,
    #[msg("A member of the council cannot be the borrower")]
    BorrowerIsCurator,
    #[msg("The endorsee is missing, or is the pool, the council, a curator, the guardian or the borrower")]
    EndorseeInvalid,

    // Amounts and invariants.
    #[msg("Amount is zero or exceeds what is due")]
    WrongAmount,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("A pool invariant does not hold after this instruction; the transaction is refused")]
    InvariantBroken,
    #[msg("Not enough liquidity in the vault")]
    InsufficientLiquidity,
    #[msg("The deposit would mint zero shares")]
    ZeroShares,
    #[msg("First deposit below the minimum")]
    FirstDepositTooSmall,
    #[msg("The result is below the minimum the caller accepts")]
    SlippageExceeded,
    #[msg("The principal exceeds the pool's maximum line, max_line_bps of the NAV")]
    LineTooLarge,
    #[msg("The draw would take the pool's outstanding principal above its cap")]
    OutstandingCapExceeded,

    // Pool parameters.
    #[msg("The collection period is shorter than the minimum")]
    CollectionPeriodTooShort,
    #[msg("The acceleration notice is below the seven-day floor")]
    AccelerationNoticeTooShort,
    #[msg("The borrower's cancel grace is below the thirty-day floor")]
    CancelGraceTooShort,

    // Lines.
    #[msg("The line is not in the required status")]
    LineStatus,
    #[msg("The signed agreement's hash is required")]
    AgreementHashRequired,
    #[msg("The expediente root is required")]
    ExpedienteRootRequired,
    #[msg("The pagaré template's hash is required")]
    PagareTemplateHashRequired,
    #[msg("The expediente root or the terms hash is not the one the borrower requested")]
    ExpedienteMismatch,
    #[msg("The request is older than the council's review window")]
    RequestExpired,
    #[msg("sponsor_role is unknown")]
    SponsorRoleInvalid,
    #[msg("The term has ended, or would end before the opening")]
    TermEnded,
    #[msg("The installment schedule is not valid")]
    InvalidSchedule,
    #[msg("max_missed is below the floor: at least one missed installment must come before the collection trigger")]
    MaxMissedTooLow,
    #[msg("The last installment would fall due after the term")]
    ScheduleEndsAfterTerm,
    #[msg("The line has no installment schedule")]
    NotScheduled,
    #[msg("The draw window of this line has closed")]
    AvailabilityEnded,
    #[msg("The line has reached its maximum number of draws")]
    TooManyDraws,
    #[msg("The line's draws are not suspended")]
    DrawsNotSuspended,
    #[msg("The line's draw window has already closed")]
    WindowClosed,
    #[msg("No acceleration is pending on this line, or one already is")]
    AccelerationState,
    #[msg("The acceleration notice has not run")]
    AccelerationNotDue,
    #[msg("The repayment must cover the late interest due")]
    LateInterestUnpaid,
    #[msg("Fewer installments missed than the line allows before collection")]
    NotEnoughMissed,
    #[msg("The term has not ended")]
    TermNotEnded,
    #[msg("A line in collection is written off only after its collection period, and after its oldest missed due date or its term, plus grace")]
    WriteOffTooEarly,

    // The FIX.
    #[msg("The FIX date is not a valid date, is in the future, or is older than the published one")]
    FixDateInvalid,
    #[msg("The FIX moves more than the feeder may publish; the council must")]
    FixJump,
    #[msg("The FIX signed is not the one published now")]
    FixMismatch,
    #[msg("No FIX is published, or it is too old for a draw")]
    FixStale,

    // Pagarés.
    #[msg("The signed document's hash is required")]
    DocumentHashRequired,
    #[msg("signature_method is unknown, or lacks its certificate fingerprint")]
    SignatureMethodInvalid,
    #[msg("No repayment has marked this pagaré paid")]
    PagareNotCovered,
    #[msg("This pagaré is already extinguished")]
    PagareAlreadyPaid,
    #[msg("The borrower may cancel alone only after the cancel grace from the payment")]
    CancelGraceNotElapsed,
    #[msg("This kind of endoso is not one the program records")]
    EndosoKindInvalid,
    #[msg("The pagaré has no procuración in force, or already has one")]
    ProcuracionState,
    #[msg("This kind is not a court annotation")]
    AnnotationKindInvalid,
    #[msg("The referenced registry entry does not exist")]
    EntryNotFound,

    // Added after the codes above were published; appended so no existing code moves.
    #[msg("A repayment takes at most MAX_SIDECARS_PER_REPAY pagaré records; split the payment")]
    TooManySidecars,
    #[msg("The borrower's cancel grace is above the one-year ceiling")]
    CancelGraceTooLong,
    #[msg("The council's decision is dated more than fifteen days ahead")]
    ProposalTooFar,
}
