use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Fields, FnArg, ImplItem, ItemImpl, ItemStruct, Visibility, parse_macro_input};

/// Annotation for impl blocks. It generates a ModuleCall implementation
/// that dispatches calls to the annotated methods based on their names and argument counts.
///
/// Example usage:
/// ```rust,no_run
/// use nova_macros::{engine_module, script_module};
///
/// #[engine_module]
/// struct MathModule;
///
/// #[script_module]
/// impl MathModule {
///     pub fn add(&self, a: i32, b: i32) -> i32 {
///         a + b
///     }
/// }
///
/// fn example() {
///     let mut math = MathModule {};
///     let mut vm: VmContext<'_> = VmContext::new()
///         .add_module(b"math", &mut math)
///         .unwrap();
///     let result = vm.run(b"import math; i = math.add(1, 2);").unwrap();
///
///     let i = result.get_var(b"i").unwrap(); // 3
/// }
/// ```
#[proc_macro_attribute]
pub fn script_module(_metadata: TokenStream, input: TokenStream) -> TokenStream {
    let input_impl = parse_macro_input!(input as ItemImpl);
    let struct_ty = &input_impl.self_ty;

    let mut call_arms = Vec::new();

    for item in &input_impl.items {
        if let ImplItem::Fn(method) = item {
            if !matches!(method.vis, Visibility::Public(_)) {
                continue;
            }
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

        #[allow(unused_qualifications)]
        const _: () = {
            use ::nova::__private::{EngineObject, FromEngine, InterpreterError, ModuleCall, ToEngine};

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
        };
    })
}

/// Annotation for structs. It generates a ModuleGet implementation
/// that allows accessing the annotated fields as module members.
///
/// See [macro@script_module] for example usage.
#[proc_macro_attribute]
pub fn engine_module(_args: TokenStream, input: TokenStream) -> TokenStream {
    // Parse as ItemStruct instead of DeriveInput to handle the full struct definition
    let item_struct = parse_macro_input!(input as ItemStruct);
    let name = &item_struct.ident;

    let mut get_arms = Vec::new();

    // ItemStruct has fields directly, no need to match on Data::Struct
    if let Fields::Named(fields) = &item_struct.fields {
        for field in &fields.named {
            if !matches!(field.vis, Visibility::Public(_)) {
                continue;
            }
            let field_name = field.ident.as_ref().unwrap();
            let field_str = field_name.to_string();
            let field_bytes = syn::LitByteStr::new(field_str.as_bytes(), field_name.span());

            get_arms.push(quote! {
                #field_bytes => ToEngine::to_engine(self.#field_name),
            });
        }
    }

    TokenStream::from(quote! {
        #[allow(non_snake_case)]
        #item_struct

        #[allow(unused_qualifications)]
        const _: () = {
            use ::nova::__private::{EngineObject, InterpreterError, ModuleGet, ToEngine};

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
        };
    })
}
