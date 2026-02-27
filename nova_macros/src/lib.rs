use proc_macro::TokenStream;
use quote::{format_ident, quote};
// Note: Using DeriveInput instead of ItemStruct
use syn::{Data, DeriveInput, Fields, FnArg, ImplItem, ItemImpl, parse_macro_input};

#[proc_macro_attribute]
pub fn script_module(_metadata: TokenStream, input: TokenStream) -> TokenStream {
    let input_impl = parse_macro_input!(input as ItemImpl);
    let struct_ty = &input_impl.self_ty;

    let mut call_arms = Vec::new();

    for item in &input_impl.items {
        if let ImplItem::Fn(method) = item {
            let method_name = &method.sig.ident;
            let method_str = method_name.to_string();
            let method_bytes = syn::LitByteStr::new(method_str.as_bytes(), method_name.span());

            let inputs: Vec<_> = method
                .sig
                .inputs
                .iter()
                .filter_map(|arg| {
                    if let FnArg::Typed(pt) = arg {
                        Some(pt)
                    } else {
                        None
                    }
                })
                .collect();

            let nargs = inputs.len();

            let arg_conversions = inputs.iter().enumerate().map(|(i, pt)| {
                let ty = &pt.ty;
                let idx = format_ident!("arg_{}", i);
                quote! {
                    let #idx = <#ty as FromEngine>::from_engine(&args[#i])?;
                }
            });

            let call_args = (0..nargs).map(|i| format_ident!("arg_{}", i));

            call_arms.push(quote! {
                (#method_bytes, #nargs) => {
                    #(#arg_conversions)*
                    let result = self.#method_name(#(#call_args),*);
                    ToEngine::to_engine(result)
                }
            });
        }
    }

    TokenStream::from(quote! {
        #input_impl

        impl ModuleCall for #struct_ty {
            fn internal_call<'a>(
                &mut self,
                func: &'a [u8],
                args: &[EngineObject<'a>],
            ) -> Result<EngineObject<'a>, InterpreterError<'a>> {
                match (func, args.len()) {
                    #(#call_arms)*
                    _ => Err(
                        InterpreterError::InvalidModuleFunctionCall {
                            func,
                            nargs: args.len(),
                        }
                    ),
                }
            }
        }
    })
}

#[proc_macro_derive(EngineModule)]
pub fn derive_module_get(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let mut get_arms = Vec::new();

    if let Data::Struct(data_struct) = &input.data {
        if let Fields::Named(fields) = &data_struct.fields {
            for field in &fields.named {
                let field_name = field.ident.as_ref().unwrap();
                let field_str = field_name.to_string();
                let field_bytes = syn::LitByteStr::new(field_str.as_bytes(), field_name.span());

                get_arms.push(quote! {
                    #field_bytes => ToEngine::to_engine(self.#field_name),
                });
            }
        }
    }

    TokenStream::from(quote! {
        impl ModuleGet for #name {
            fn internal_get<'a>(
                &self,
                member: &'a [u8],
            ) -> Result<EngineObject<'a>, InterpreterError<'a>> {
                match member {
                    #(#get_arms)*
                    _ => Err(InterpreterError::InvalidModuleMemberAccess { member }),
                }
            }
        }
    })
}
