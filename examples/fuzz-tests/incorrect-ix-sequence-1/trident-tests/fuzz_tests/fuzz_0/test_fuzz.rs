use fuzz_instructions::Initialize;
use trident_client::fuzzing::*;
mod fuzz_instructions;
use fuzz_instructions::FuzzInstruction;
use incorrect_ix_sequence_1::entry as entry_incorrect_ix_sequence_1;
use incorrect_ix_sequence_1::ID as PROGRAM_ID_INCORRECT_IX_SEQUENCE_1;
const PROGRAM_NAME_INCORRECT_IX_SEQUENCE_1: &str = "incorrect_ix_sequence_1";
struct InstructionsSequence;
/// Define instruction sequences for invocation.
/// `pre` runs at the start, `middle` in the middle, and `post` at the end.
/// For example, to call `InitializeFn`, `UpdateFn` and then `WithdrawFn` during
/// each fuzzing iteration:
/// ```
/// impl FuzzDataBuilder<FuzzInstruction> for InstructionsSequence {
///     pre_sequence!(InitializeFn,UpdateFn);
///     middle_sequence!(WithdrawFn);
///}
/// ```
/// For more details, see: https://ackee.xyz/trident/docs/dev/features/instructions-sequences/#instructions-sequences
impl FuzzDataBuilder<FuzzInstruction> for InstructionsSequence {
    pre_sequence!(Initialize);
}
/// `fn fuzz_iteration` runs during every fuzzing iteration.
/// Modification is not required.
fn fuzz_iteration<T: FuzzTestExecutor<U> + std::fmt::Display, U>(
    fuzz_data: FuzzData<T, U>,
    config: &Config,
) {
    let fuzzing_program_incorrect_ix_sequence_1 = FuzzingProgram::new(
        PROGRAM_NAME_INCORRECT_IX_SEQUENCE_1,
        &PROGRAM_ID_INCORRECT_IX_SEQUENCE_1,
        processor!(convert_entry!(entry_incorrect_ix_sequence_1)),
    );
    let mut client =
        ProgramTestClientBlocking::new(&[fuzzing_program_incorrect_ix_sequence_1], config).unwrap();
    let _ = fuzz_data.run_with_runtime(&mut client, config);
}
fn main() {
    let config = Config::new();
    fuzz_trident ! (fuzz_ix : FuzzInstruction , | fuzz_data : InstructionsSequence | { fuzz_iteration (fuzz_data , & config) ; });
}
