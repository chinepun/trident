use anchor_lang_idl_spec::Idl;
use quote::{format_ident, ToTokens};
use syn::parse_quote;

pub fn generate_source_code(idl_instructions: &[Idl]) -> String {
    let program_imports = get_program_imports(idl_instructions);
    let program_names = get_program_names(idl_instructions);
    let (fuzzing_programs, programs_array) = get_fuzzing_programs(idl_instructions);

    let test_fuzz_definition: syn::File = parse_quote! {
        use trident_client::fuzzing::*;
        mod fuzz_instructions;
        use fuzz_instructions::FuzzInstruction;

        #(#program_imports)*

        #(#program_names)*

        struct InstructionsSequence;


        /// Define instruction sequences for invocation.
        /// `pre` runs at the start, `middle` in the middle, and `post` at the end.
        /// For example, to call `InitializeFn`, `UpdateFn` and then `WithdrawFn` during each fuzzing iteration:
        /// ```
        /// use fuzz_instructions::{InitializeFn, UpdateFn, WithdrawFn};
        /// impl FuzzDataBuilder<FuzzInstruction> for InstructionsSequence {
        ///     pre_sequence!(InitializeFn,UpdateFn);
        ///     middle_sequence!(WithdrawFn);
        ///}
        /// ```
        /// For more details, see: https://ackee.xyz/trident/docs/latest/features/instructions-sequences/#instructions-sequences
        impl FuzzDataBuilder<FuzzInstruction> for InstructionsSequence {}

        /// `fn fuzz_iteration` runs during every fuzzing iteration.
        /// Modification is not required.
        fn fuzz_iteration<T: FuzzTestExecutor<U> + std::fmt::Display, U>(fuzz_data: FuzzData<T, U>,config: &Config) {


            #(#fuzzing_programs)*

            let mut client = ProgramTestClientBlocking::new(&#programs_array,config).unwrap();

            let _ = fuzz_data.run_with_runtime(&mut client,config);

        }

        fn main() {
            let config = Config::new();
            fuzz_trident!(fuzz_ix: FuzzInstruction, |fuzz_data: InstructionsSequence| {

                fuzz_iteration(fuzz_data,&config);

            });
        }
    };

    test_fuzz_definition.into_token_stream().to_string()
}

fn get_program_names(idl_instructions: &[Idl]) -> Vec<syn::Stmt> {
    idl_instructions
        .iter()
        .map(|idl| {
            let program_name = &idl.metadata.name;
            let program_name_upper = &idl.metadata.name;
            let program_name_ident = format_ident!("PROGRAM_NAME_{}", program_name_upper);

            parse_quote!(const #program_name_ident: &str = #program_name;)
        })
        .collect()
}

fn get_program_imports(idl_instructions: &[Idl]) -> Vec<syn::ItemUse> {
    idl_instructions
        .iter()
        .flat_map(|idl| {
            let program_name = &idl.metadata.name;
            let program_name_upper = &idl.metadata.name;
            let program_name_ident = format_ident!("{}", program_name);
            let program_entry_ident = format_ident!("entry_{}", program_name);
            let program_id_name_ident = format_ident!("PROGRAM_ID_{}", program_name_upper);

            vec![
                parse_quote!(use #program_name_ident::entry as #program_entry_ident;),
                parse_quote!(use #program_name_ident::ID as #program_id_name_ident;),
            ]
        })
        .collect()
}

fn get_fuzzing_programs(idl_instructions: &[Idl]) -> (Vec<syn::Stmt>, syn::ExprArray) {
    // Vector to collect the `fuzzing_program_name_ident` identifiers
    let mut fuzzing_program_idents = Vec::new();

    // Collect the statements and the identifiers for the `ExprArray`
    let fuzzing_programs: Vec<syn::Stmt> = idl_instructions
        .iter()
        .map(|idl| {
            let program_name = &idl.metadata.name;
            let program_name_upper = &idl.metadata.name;
            let fuzzing_program_name_ident = format_ident!("fuzzing_program_{}", program_name);
            let program_id_name_ident = format_ident!("PROGRAM_ID_{}", program_name_upper);
            let program_name_ident = format_ident!("PROGRAM_NAME_{}", program_name_upper);
            let program_entry_ident = format_ident!("entry_{}", program_name);

            // Add the identifier to the vector
            fuzzing_program_idents.push(fuzzing_program_name_ident.clone());

            // Generate the statement
            parse_quote! {
                let #fuzzing_program_name_ident = FuzzingProgram::new(
                    #program_name_ident,
                    &#program_id_name_ident,
                    processor!(convert_entry!(#program_entry_ident))
                );
            }
        })
        .collect();

    // Create the ExprArray from the collected identifiers
    let expr_array: syn::ExprArray = syn::ExprArray {
        attrs: vec![],
        bracket_token: syn::token::Bracket::default(),
        elems: fuzzing_program_idents
            .into_iter()
            .map(|ident| {
                syn::Expr::Path(syn::ExprPath {
                    attrs: vec![],
                    qself: None,
                    path: ident.into(),
                })
            })
            .collect(),
    };

    // Return the vector of statements and the ExprArray
    (fuzzing_programs, expr_array)
}
