/// This is a system for automatically generating getters/setters for
/// the Atmosphere ECS.
///
/// Austin Shafer - 2020

extern crate proc_macro;
extern crate proc_macro2;
use proc_macro::TokenStream;
use quote::quote;
use syn::DeriveInput;
use syn;

// Resources used:
//
// https://riptutorial.com/rust/example/28271/getters-and-setters
// https://doc.rust-lang.org/book/ch19-06-macros.html

fn construct_getters(variants: &Vec<&syn::Variant>,
                     name: &String,
                     enum_ident: &syn::Ident)
                     -> Vec<proc_macro2::TokenStream>
{
    variants.iter().map(|ref v| {
        // the variant identifier
        let ident = &v.ident;
        // create a 'get_*' identifier
        let get_ident = syn::parse_str::<syn::Ident>(
            format!("get_{}", ident).as_str()
        ).expect("Could not parse get_ident");
        // create an identifier for the propery equivalent of this
        // variant
        let prop_ident = syn::parse_str::<syn::Ident>(
            format!("{}", ident).to_uppercase().as_str()
        ).expect("Could not parse get_ident");

        // get a vec of types, one for each field in this variant
        let tys: Vec<_> = v.fields.iter().map(|ref f| {
            f.ty.clone()
        }).collect();
        // get a vec of arg names (arg0, arg1...)
        let argnames: Vec<_> = tys.iter().enumerate().map(|(i, _)| {
            syn::parse_str::<syn::Ident>(
                &format!("arg{}", i).as_str()).unwrap()
        }).collect();

        // We need to adjust the return type based on what fields
        // this variant has. If it only has one field, we can just return
        // that type. If it has more, we need to construct a tuple to return
        // note that we have to clone returns
        let (ret_type, ret) = if tys.len() == 1 {
            let ty = &tys[0];
            (quote! { #ty }, quote! { arg0.clone() })
        } else if tys.len() > 1 {
            (quote! { ( #(#tys),* ) }, quote! { (#(#argnames.clone()),*) })
        } else {
            panic!("all ECS variants need to have at least one field");
        };

        // We need to adjust the `set_*_prop` call to be generated by
        // these getters/setters. Different property types have different
        // calls and different argument requirements
        let (matchline, args) = if name == "GlobalProperty" {
            (quote! { self.get_global_prop(#enum_ident::#prop_ident) },
             quote! {})
        } else if name == "WindowProperty" {
            (quote! { self.get_window_prop(id, #enum_ident::#prop_ident) },
             quote! { id: WindowId })
        } else if name == "ClientProperty" {
            (quote! { self.get_client_prop(id, #enum_ident::#prop_ident) },
             quote! { id: ClientId })
        } else {
            panic!("#[derive(AtmosECSGetSet)] unrecognized atmos id type");
        };

        // Finally we can construct a getter function for this variant
        quote! {
            pub fn #get_ident(&self, #args) -> #ret_type {
                match #matchline {
                    Some(#enum_ident::#ident(#(#argnames),*)) => #ret,
                    _ => panic!("property not found"),
                }
            }
        }
    }).collect()
}

fn construct_properties(variants: &Vec<&syn::Variant>,
                        enum_ident: &syn::Ident)
                        -> proc_macro2::TokenStream
{
    let mut i: usize = 0;
    let mut props = Vec::new();
    let mut matches = Vec::new();

    for v in variants.iter() {
        // the variant identifier
        let ident = &v.ident;
        // create an identifier for the property equivalent of this
        // variant
        let prop_ident = syn::parse_str::<syn::Ident>(
            format!("{}", ident).to_uppercase().as_str()
        ).expect("Could not parse get_ident");

        let underscores: Vec<_> = (0..v.fields.len()).map(|i| {
            syn::parse_str::<syn::Ident>(&format!("_arg{:?}",i).to_string())
                .unwrap()
        }).collect();

        // constants
        props.push(quote! {
            const #prop_ident: PropertyId = #i;
        });

        // these are for get_property_id
        matches.push(quote! {
            Self::#ident(#(#underscores),*) => Self::#prop_ident,
        });
        i += 1;
    }

    // this one is for telling how many properties there are
    props.push(quote! {
        const VARIANT_LEN: PropertyId = #i;
    });

    quote! {
        // Declare constants for the property ids. This prevents us
        // from having to make an instance of the enum that we would
        // have to call get_property_id on
        impl #enum_ident {
            #(#props)*
        }

        impl Property for #enum_ident {
            // Get a unique Id
            fn get_property_id(&self) -> PropertyId {
                match self {
                    #(#matches)*
                }
            }
            
            fn variant_len() -> u32 {
                return Self::VARIANT_LEN as u32;
            }
        }
    }
}

fn construct_setters(variants: &Vec<&syn::Variant>,
                     name: &String,
                     enum_ident: &syn::Ident)
                     -> Vec<proc_macro2::TokenStream>
{
    variants.iter().map(|ref v| {
        // the variant identifier
        let ident = &v.ident;
        // create a 'get_*' identifier
        let set_ident = syn::parse_str::<syn::Ident>(
            format!("set_{}", ident).as_str()
        ).expect("Could not parse get_ident");

        // get a vec of types, one for each field in this variant
        let tys: Vec<_> = v.fields.iter().map(|ref f| {
            f.ty.clone()
        }).collect();
        // get a vec of arg names (arg0, arg1...)
        let argnames: Vec<_> = tys.iter().enumerate().map(|(i, _)| {
            syn::parse_str::<syn::Ident>(
                &format!("arg{}", i).as_str()).unwrap()
        }).collect();

        // get the arguments+types for the function decl
        let mut arglist: Vec<_> = argnames.iter().enumerate().map(|(i, a)| {
            let ty = &tys[i];
            quote! { #a: #ty }
        }).collect();

        // make something of the form &WindowProperty::window_dimensions(...)
        let prop = quote! {
            &#enum_ident::#ident(
                #(#argnames),*
            )
        };

        // We need to adjust the `set_*_prop` call to be generated by
        // these getters/setters. Different property types have different
        // calls and different argument requirements
        let reqline = if name == "GlobalProperty" {
            quote! { self.set_global_prop(#prop); }
        } else if name == "WindowProperty" {
            arglist.insert(0, quote!{ id: WindowId });
            quote! { self.set_window_prop(id, #prop); }
        } else if name == "ClientProperty" {
            arglist.insert(0, quote!{ id: ClientId });
            quote! { self.set_client_prop(id, #prop); }
        } else {
            panic!("#[derive(AtmosECSGetSet)] unrecognized atmos id type");
        };

        // Finally we can construct a getter function for this variant
        quote! {
            pub fn #set_ident(&mut self, #(#arglist),*) {
                #reqline
            }
        }
    }).collect()
}

/// Macro for generating getters/setters for ECS fields
#[proc_macro_derive(AtmosECSGetSet)]
pub fn ecs_get_set(input: TokenStream) -> TokenStream {
    // Construct a representation of Rust code as a syntax tree
    // that we can manipulate
    let ast: DeriveInput = syn::parse(input).unwrap();
    let name: String = ast.ident.to_string();
    let enum_ident = &ast.ident;

    // get the enum token
    if let syn::Data::Enum(e) = ast.data {
        // get all variants in this enum
        let variants: Vec<_> = e.variants.iter().collect();

        // Now we will construct a set of getter functions
        // for each variant
        let getters = construct_getters(&variants, &name, &enum_ident);
        let setters = construct_setters(&variants, &name, &enum_ident);
        let props = construct_properties(&variants, &enum_ident);

        let gen = quote!{
            impl Atmosphere {
                #(
                    #getters
                    #setters
                )*
            }
            #props
        };
        //println!("{:#?}", gen);
        return gen.into();
    }
    // not an enum, so don't generate any code
    panic!("#[derive(AtmosECSGetSet)] can only be used with known atmos id types");
}