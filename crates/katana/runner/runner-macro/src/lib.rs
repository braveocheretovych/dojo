mod args;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, parse_quote, Stmt};

#[proc_macro_attribute]
pub fn katana_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as args::MacroArgs);
    let mut test_function = parse_macro_input!(item as syn::ItemFn);
    let function_name = test_function.sig.ident.to_string();

    let n_accounts = args.accounts.unwrap_or(2);
    let block_time = args.block_time.map(|b| quote!(Some(#b))).unwrap_or(quote!(None));
    let program_name = args.path.map(|e| quote!(Some(String::from(#e)))).unwrap_or(quote!(None));

    let header: Stmt = parse_quote! {
        let runner =
            katana_runner::KatanaRunner::new_with_config(
                katana_runner::KatanaRunnerConfig {
                    program_name: #program_name,
                    run_name: Some(String::from(#function_name)),
                    block_time: #block_time,
                    n_accounts: #n_accounts,
                    ..Default::default()
                }
            )
                .expect("failed to start katana");
    };

    test_function.block.stmts.insert(0, header);

    if test_function.sig.asyncness.is_none() {
        TokenStream::from(quote! {
            #[test]
            #test_function
        })
    } else {
        TokenStream::from(quote! {
            #[tokio::test]
            #test_function
        })
    }
}

#[proc_macro]
pub fn runner(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as args::MacroArgs);
    let function_name = quote!(function_name);

    let n_accounts = args.accounts.unwrap_or(2);
    let block_time = args.block_time.map(|b| quote!(Some(#b))).unwrap_or(quote!(None));
    let program_name = args.path.map(|e| quote!(Some(String::from(#e)))).unwrap_or(quote!(None));

    TokenStream::from(quote! {
        lazy_static::lazy_static! {
            pub static ref RUNNER: std::sync::Arc<katana_runner::KatanaRunner> = std::sync::Arc::new(
                katana_runner::KatanaRunner::new_with_config(
                    katana_runner::KatanaRunnerConfig {
                        program_name: #program_name,
                        run_name: Some(String::from(#function_name)),
                        block_time: #block_time,
                        n_accounts: #n_accounts,
                        ..Default::default()
                    }
                )
                    .expect("failed to start katana")
            );
        }

        let runner = &RUNNER;
    })
}
