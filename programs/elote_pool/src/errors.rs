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
    #[msg("The term has ended, or would end before the opening")]
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
    #[msg("The pool admin cannot be the borrower")]
    BorrowerIsAdmin,
    #[msg("The principal exceeds the pool's maximum line, max_line_bps of the NAV")]
    LineTooLarge,
    #[msg("A pool invariant does not hold after this instruction; the transaction is refused")]
    InvariantBroken,
    #[msg("The asset mint carries a Token-2022 extension the pool does not accept")]
    AssetMintNotAllowed,
    #[msg("The share mint is not one this pool can own and price: wrong authority, supply, decimals or extensions")]
    ShareMintNotAllowed,
    #[msg("The pool's attestor must sign")]
    AttestorRequired,
    #[msg("The draw window of this line has closed")]
    AvailabilityEnded,
    #[msg("The line has reached its maximum number of draws")]
    TooManyDraws,
    #[msg("The collection is not a Metaplex Core collection whose update authority is the pool")]
    CollectionNotAllowed,
    #[msg("The signed pagaré's hash is required")]
    DocumentHashRequired,
    #[msg("signature_method is unknown, or lacks its certificate fingerprint")]
    SignatureMethodInvalid,
    #[msg("The line has not repaid enough to cover this pagaré")]
    PagareNotCovered,
    #[msg("This pagaré is already marked paid")]
    PagareAlreadyPaid,
    #[msg("The pagaré's holder must sign")]
    NotHolder,
    #[msg("The attestor cannot be the admin")]
    AttestorIsAdmin,
    #[msg("The borrower cannot be the attestor")]
    BorrowerIsAttestor,
    #[msg("The signed agreement's hash is required")]
    AgreementHashRequired,
    #[msg("The expediente root is required")]
    ExpedienteRootRequired,
    #[msg("sponsor_role is unknown")]
    SponsorRoleInvalid,
    #[msg("The collection period is shorter than the minimum")]
    CollectionPeriodTooShort,
    #[msg("The sidecars passed are not the ones this payment touches, oldest first")]
    PagareAccountsMismatch,
    #[msg("This kind of endoso is not one the program records, or not in this version")]
    EndosoKindInvalid,
    #[msg("The endorsee is missing or is the pool itself")]
    EndorseeInvalid,
    #[msg("The pagaré has no procuración in force, or already has one")]
    ProcuracionState,
    #[msg("This kind is not a court annotation")]
    AnnotationKindInvalid,
    #[msg("The cancel delay is below the seven-day floor")]
    CancelDelayTooShort,
    #[msg("A cancellation request is already pending on this pagaré")]
    CancellationPending,
    #[msg("The referenced registry entry does not exist")]
    EntryNotFound,
}
