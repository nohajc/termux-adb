use proc_macro2::{TokenStream, TokenTree, Literal, Delimiter, Group};
use quote::{quote, format_ident, TokenStreamExt};

/*

type CloseFn = unsafe extern "C" fn(c_int) -> c_int;
static REAL_CLOSE: Lazy<CloseFn> = Lazy::new(|| func!(LIBC, close));

 */

#[proc_macro_attribute]
pub fn define_hook(attribute: proc_macro::TokenStream, function: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let attribute = TokenStream::from(attribute);
    let function = TokenStream::from(function);

    let attr0 = attribute.into_iter().next()
        .expect("#[define_hook] missing target library attribute");

    let _target_lib = match attr0 {
        TokenTree::Ident(ident) => ident,
        _ => panic!("#[define_hook] target library attribute must be a valid identifier")
    };

    let mut fn_item = String::from("");
    let mut fnptr_type = TokenStream::new();

    let mut last_tok = TokenTree::from(Literal::i32_unsuffixed(0));

    for ref tok in function.clone() {
        if let TokenTree::Ident(ident) = last_tok {
            if ident.to_string() == "fn" {
                fn_item = tok.to_string();
                last_tok = tok.to_owned();
                continue;
            }
        }

        let mut new_tok = tok.to_owned();
        match tok {
            TokenTree::Group(group) => {
                if group.delimiter() == Delimiter::Parenthesis {
                    let mut gs = TokenStream::new();
                    let mut skipping = true;

                    for ref tok in group.stream() {
                        // println!("DEBUG idx = {}, tok = {:?}", idx, &tok);
                        if !skipping {
                            gs.append(tok.to_owned());
                        }

                        match tok {
                            TokenTree::Punct(punct) => {
                                if punct.as_char() == ',' {
                                    skipping = true;
                                } else if punct.as_char() == ':' {
                                    skipping = false;
                                }
                            }
                            _ => ()
                        }
                    }

                    new_tok = TokenTree::Group(Group::new(Delimiter::Parenthesis, gs));
                }
                if group.delimiter() == Delimiter::Brace {
                    break;
                }
            }
            TokenTree::Ident(ident) => {
                if ident.to_string() == "pub" {
                    last_tok = tok.to_owned();
                    continue;
                }
            }
            _ => ()
        }

        fnptr_type.append(new_tok.clone());
        last_tok = new_tok;
    }

    let fnptr_type_alias = format_ident!("{}_fn", &fn_item);

    let result = quote! {
        type #fnptr_type_alias = #fnptr_type;

        #function
    };

    // println!("define_hook fn: {:?}", function);
    result.into()
}
