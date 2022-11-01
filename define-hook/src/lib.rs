use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn define_hook(_attribute: TokenStream, function: TokenStream) -> TokenStream {
    println!("define_hook: {:?}", function);
    function
}
