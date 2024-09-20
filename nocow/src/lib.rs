use proc_macro2::Ident;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    AngleBracketedGenericArguments, Data, DataStruct, DeriveInput, Fields, GenericArgument,
    PathArguments,
};

struct NoCow {
    name: Ident,
}

impl Parse for NoCow {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let struct_name = input.parse::<syn::Ident>()?;
        Ok(NoCow { name: struct_name })
    }
}

#[proc_macro_attribute]
pub fn nocow(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let attr = syn::parse_macro_input!(attr as NoCow);
    let input: DeriveInput = syn::parse_macro_input!(input as DeriveInput);
    match &input.data {
        Data::Struct(_) => {
            let mut nocow_struct: DeriveInput = input.clone();
            nocow_struct.ident = attr.name;
            if let Some(syn::GenericParam::Lifetime(_)) = nocow_struct.generics.params.first() {
                nocow_struct.generics.params =
                    nocow_struct.generics.params.into_iter().skip(1).collect();
            }
            if let Data::Struct(DataStruct {
                fields: Fields::Named(fields),
                ..
            }) = &mut nocow_struct.data
            {
                for f in fields.named.iter_mut() {
                    if let syn::Type::Path(syn::TypePath {
                        qself: None,
                        ref mut path,
                    }) = f.ty
                    {
                        if path.segments[0].ident == "Cow" {
                            if let PathArguments::AngleBracketed(AngleBracketedGenericArguments {
                                args,
                                ..
                            }) = path.segments[0].arguments.clone()
                            {
                                if let GenericArgument::Type(ty) = args.last().unwrap() {
                                    f.ty = ty.clone();
                                }
                            }
                        }
                    }
                }
            }

            let structs = [quote!( #input ), quote!( #nocow_struct )];
            let expanded = quote!(
                #( #structs )*
            );
            proc_macro::TokenStream::from(expanded)
        }
        _ => panic!("expected struct"),
    }
}
