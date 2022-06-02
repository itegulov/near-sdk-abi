use convert_case::{Case, Casing};
use near_sdk::__private::AbiRoot;
use quote::{format_ident, quote};
use schemafy_lib::{Generator, Schema};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[proc_macro]
pub fn near_abi(tokens: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let def = syn::parse_macro_input!(tokens as Def);
    let input_file_str = def.input_file.value();
    let input_file = Path::new(&input_file_str);
    let input_file = if input_file.is_relative() {
        let crate_root = get_crate_root().unwrap();
        crate_root.join(&input_file_str)
    } else {
        PathBuf::from(input_file)
    };

    let abi_json = std::fs::read_to_string(&input_file)
        .unwrap_or_else(|err| panic!("Unable to read `{}`: {}", input_file.to_string_lossy(), err));

    let near_abi = serde_json::from_str::<AbiRoot>(&abi_json).expect("invalid NEAR ABI");
    let contract_name = near_abi
        .metainfo
        .name
        .map(|n| format_ident!("Ext{}", n.to_case(Case::UpperCamel)))
        .or(def.contract_name.map(|n| format_ident!("{}", n)))
        .unwrap_or(format_ident!("ExtContract"));

    let schema_json = serde_json::to_string(&near_abi.abi.root_schema).unwrap();

    let mut generator = Generator::builder().with_input_json(schema_json).build();
    let (mut token_stream, mut expander) = generator.generate();

    let mut registry = HashMap::<u32, String>::new();
    for t in near_abi.abi.types {
        let schema_json = serde_json::to_string(&t.schema).unwrap();
        let schema: Schema = serde_json::from_str(&schema_json).unwrap_or_else(|err| {
            panic!(
                "Cannot parse `{}` as JSON: {}",
                input_file.to_string_lossy(),
                err
            )
        });
        let field_type = expander.expand_type_from_schema(&schema).typ.clone();

        registry.insert(t.id, field_type);
    }

    let methods = near_abi
        .abi
        .functions
        .iter()
        .map(|m| {
            let name = format_ident!("{}", m.name);
            let result_type = m
                .result
                .clone()
                .map(|r_param| {
                    let r_type = format_ident!(
                        "{}",
                        registry
                            .get(&r_param.type_id)
                            .expect("Unexpected result type")
                    );
                    quote! { -> #r_type }
                })
                .unwrap_or_else(|| quote! {});
            let args = m
                .params
                .iter()
                .enumerate()
                .map(|(i, a_param)| {
                    let a_type = format_ident!(
                        "{}",
                        registry
                            .get(&a_param.type_id)
                            .expect("Unexpected argument type")
                    );
                    let a_name = format_ident!("arg{}", &i);
                    quote! { #a_name: #a_type }
                })
                .collect::<Vec<_>>();
            quote! { fn #name(&self, #(#args),*) #result_type; }
        })
        .collect::<Vec<_>>();

    token_stream.extend(quote! {
        #[near_sdk::ext_contract]
        pub trait #contract_name {
            #(#methods)*
        }
    });

    token_stream.into()
}

struct Def {
    contract_name: Option<String>,
    input_file: syn::LitStr,
}

impl syn::parse::Parse for Def {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let contract_name = if input.peek(syn::Ident) {
            let contract_name_ident: syn::Ident = input.parse()?;
            if contract_name_ident != "contract_name" {
                return Err(syn::Error::new(
                    contract_name_ident.span(),
                    "Expected `contract_name`",
                ));
            }
            input.parse::<syn::Token![:]>()?;
            Some(input.parse::<syn::Ident>()?.to_string())
        } else {
            None
        };
        Ok(Def {
            contract_name,
            input_file: input.parse()?,
        })
    }
}

fn get_crate_root() -> std::io::Result<PathBuf> {
    if let Ok(path) = std::env::var("CARGO_MANIFEST_DIR") {
        return Ok(PathBuf::from(path));
    }

    let current_dir = std::env::current_dir()?;

    for p in current_dir.ancestors() {
        if std::fs::read_dir(p)?
            .into_iter()
            .filter_map(Result::ok)
            .any(|p| p.file_name().eq("Cargo.toml"))
        {
            return Ok(PathBuf::from(p));
        }
    }

    Ok(current_dir)
}
