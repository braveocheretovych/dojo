use syn::parse::{Parse, ParseStream};
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
