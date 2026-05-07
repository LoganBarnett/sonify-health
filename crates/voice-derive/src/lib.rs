//! Derive macro for patch parameter metadata and accessors.
//!
//! `#[derive(PatchGenerate)]` on a struct with `#[patch_param(...)]`
//! annotations generates:
//!
//! - `PARAMS` constant (`&'static [PatchParamMeta]`)
//! - `PatchOverrides` struct with `Option<f64>` mirrors
//! - `with_overrides()` method
//! - `set_param()` / `get_param()` methods
//! - `PatchOverrides::to_fields()` method
//!
//! Compile-time checks:
//! - Every `patch_param` field must be `f64`.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

mod patch_param;
use patch_param::PatchField;

/// Derive patch parameter metadata and accessors.
#[proc_macro_derive(PatchGenerate, attributes(patch_param))]
pub fn derive_voice_generate(input: TokenStream) -> TokenStream {
  let input = parse_macro_input!(input as DeriveInput);
  match expand(input) {
    Ok(ts) => ts.into(),
    Err(e) => e.to_compile_error().into(),
  }
}

fn expand(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
  let name = &input.ident;
  let fields = match &input.data {
    syn::Data::Struct(s) => match &s.fields {
      syn::Fields::Named(f) => &f.named,
      _ => {
        return Err(syn::Error::new_spanned(
          &input,
          "PatchGenerate requires a struct with named \
           fields",
        ))
      }
    },
    _ => {
      return Err(syn::Error::new_spanned(
        &input,
        "PatchGenerate can only be derived for structs",
      ))
    }
  };

  let mut voice_fields: Vec<PatchField> = Vec::new();

  for field in fields {
    if let Some(vf) = PatchField::from_field(field)? {
      voice_fields.push(vf);
    }
  }

  let params_const = gen_params_const(name, &voice_fields);
  let set_get_param = gen_set_get_param(name, &voice_fields);
  let with_overrides = gen_with_overrides(name, &voice_fields);
  let overrides_struct = gen_overrides_struct(&voice_fields);

  Ok(quote! {
    #params_const
    #set_get_param
    #with_overrides
    #overrides_struct
  })
}

fn gen_params_const(
  name: &syn::Ident,
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let entries: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let field_name = vf.ident.to_string();
      let min = vf.min;
      let max = vf.max;
      let step = vf.step;
      let desc = &vf.description;
      quote! {
        PatchParamMeta {
          name: #field_name,
          description: #desc,
          min: #min,
          max: #max,
          step: #step,
        }
      }
    })
    .collect();

  let count = entries.len();

  quote! {
    impl #name {
      /// Metadata for all `#[patch_param]` fields.
      pub const PARAMS: &'static [PatchParamMeta; #count] = &[
        #(#entries,)*
      ];
    }
  }
}

fn gen_set_get_param(
  name: &syn::Ident,
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let set_arms: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      let field_name = vf.ident.to_string();
      quote! {
        #field_name => self.#ident = value,
      }
    })
    .collect();

  let get_arms: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      let field_name = vf.ident.to_string();
      quote! {
        #field_name => Some(self.#ident),
      }
    })
    .collect();

  quote! {
    impl #name {
      /// Set a patch parameter by name.  Returns `true`
      /// if the name matched a known parameter.
      pub fn set_param(&mut self, name: &str, value: f64) -> bool {
        match name {
          #(#set_arms)*
          _ => return false,
        }
        true
      }

      /// Get a patch parameter by name.
      pub fn get_param(&self, name: &str) -> Option<f64> {
        match name {
          #(#get_arms)*
          _ => None,
        }
      }
    }
  }
}

fn gen_with_overrides(
  name: &syn::Ident,
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let override_stmts: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      quote! {
        if let Some(v) = o.#ident {
          self.#ident = v;
        }
      }
    })
    .collect();

  quote! {
    impl #name {
      /// Apply overrides, replacing only the specified fields.
      pub fn with_overrides(mut self, o: &PatchOverrides) -> Self {
        #(#override_stmts)*
        self
      }
    }
  }
}

fn gen_overrides_struct(
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let fields: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      quote! {
        pub #ident: Option<f64>
      }
    })
    .collect();

  let to_fields_arms: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      let field_name = vf.ident.to_string();
      quote! {
        if let Some(v) = self.#ident {
          out.push((#field_name, v));
        }
      }
    })
    .collect();

  quote! {
    /// Optional overrides for patch parameters from configuration.
    #[derive(Debug, Clone, Default, ::serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct PatchOverrides {
      #(#fields,)*
    }

    impl PatchOverrides {
      /// Extract `Some` values into a list of (name, value) pairs.
      pub fn to_fields(&self) -> Vec<(&'static str, f64)> {
        let mut out = Vec::new();
        #(#to_fields_arms)*
        out
      }
    }
  }
}
