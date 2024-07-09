use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, parse_quote, Stmt};
use syn::{Ident, LitInt, LitStr, Result, Token};

/// Default runner block interval
const DEFAULT_BLOCK_TIME: u64 = 3000; // 3 seconds

pub(crate) struct MacroArgs {
    pub(crate) path: Option<String>,
    pub(crate) accounts: Option<u16>,
    pub(crate) block_time: Option<u64>,
}

impl Parse for MacroArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut path = None;
        let mut accounts = None;
        let mut block_time = None;

        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            input.parse::<Token![=]>()?;

            match ident.to_string().as_str() {
                "accounts" => {
                    accounts = Some(input.parse::<LitInt>()?.base10_parse()?);
                }

                "block_time" => {
                    let lit = input.parse()?;
                    match lit {
                        syn::Lit::Int(lit_int) => block_time = Some(lit_int.base10_parse()?),
                        syn::Lit::Bool(lit_bool) => {
                            if lit_bool.value {
                                block_time = Some(DEFAULT_BLOCK_TIME);
                            } else {
                                block_time = None;
                            }
                        }
                        _ => return Err(input.error("Expected integer or boolean for block_time")),
                    }
                }

                "path" => {
                    path = Some(input.parse::<LitStr>()?.value());
                }

                _ => return Err(input.error("Unknown argument")),
            }

            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(MacroArgs { accounts, block_time, path })
    }
}

pub(crate) fn test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as MacroArgs);
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

pub fn runner(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as MacroArgs);
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
