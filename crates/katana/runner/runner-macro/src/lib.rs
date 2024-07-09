mod entry;

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn katana_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    entry::test(attr, item)
}

#[proc_macro]
pub fn runner(input: TokenStream) -> TokenStream {
    entry::runner(input)
}
