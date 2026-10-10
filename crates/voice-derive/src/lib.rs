//! Derive macro for patch parameter metadata and accessors.
//!
//! `#[derive(PatchGenerate)]` generates a struct's parameter table and
//! accessors from its fields' `#[patch_param(...)]` attributes.  What a hard
//! limit does to a value is hand-written in the lib crate on `PatchParamMeta`;
//! the generated code only routes each field to its entry.
//!
//! Compile-time checks: every `patch_param` field must be `f64`, and its
//! declared bounds must be consistent (see `PatchField::broken_rule`).

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
  let default_impl = gen_default(name, &voice_fields);
  let set_get_param = gen_set_get_param(name, &voice_fields);
  let positional = gen_positional(name, &voice_fields);
  let with_overrides = gen_with_overrides(name, &voice_fields);
  let overrides_struct = gen_overrides_struct(&voice_fields);

  Ok(quote! {
    #params_const
    #default_impl
    #set_get_param
    #positional
    #with_overrides
    #overrides_struct
  })
}

fn quote_option(value: Option<f64>) -> proc_macro2::TokenStream {
  value.map_or_else(
    || quote! { ::core::option::Option::None },
    |v| quote! { ::core::option::Option::Some(#v) },
  )
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
      let logarithmic = vf.logarithmic;
      let default = vf.default;
      let limit_min = quote_option(vf.limit_min);
      let limit_max = quote_option(vf.limit_max);
      let positive = vf.positive;
      let desc = &vf.description;
      quote! {
        PatchParamMeta {
          name: #field_name,
          description: #desc,
          min: #min,
          max: #max,
          step: #step,
          logarithmic: #logarithmic,
          default: #default,
          limit_min: #limit_min,
          limit_max: #limit_max,
          positive: #positive,
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

fn gen_default(
  name: &syn::Ident,
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let fields: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      let default = vf.default;
      quote! { #ident: #default }
    })
    .collect();

  quote! {
    impl ::core::default::Default for #name {
      fn default() -> Self {
        Self {
          #(#fields,)*
        }
      }
    }
  }
}

fn gen_set_get_param(
  name: &syn::Ident,
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let set_arms: Vec<_> = voice_fields
    .iter()
    .enumerate()
    .map(|(index, vf)| {
      let ident = &vf.ident;
      let field_name = vf.ident.to_string();
      quote! {
        #field_name => {
          self.#ident = Self::PARAMS[#index].limit(value).0;
          true
        }
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
      /// Set a patch parameter by name, held to the parameter's hard limits
      /// (not its slider range).  Returns `true` if the name matched a known
      /// parameter; a non-finite value is rejected the same way an unknown
      /// name is.
      pub fn set_param(&mut self, name: &str, value: f64) -> bool {
        value.is_finite()
          && match name {
            #(#set_arms)*
            _ => false,
          }
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

/// Accessors that address every parameter by its position in `PARAMS`, so a
/// caller can walk all of them without naming each one or handling a name
/// that does not match.
fn gen_positional(
  name: &syn::Ident,
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let count = voice_fields.len();
  let values: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      quote! { self.#ident }
    })
    .collect();
  let mapped: Vec<_> = voice_fields
    .iter()
    .enumerate()
    .map(|(index, vf)| {
      let ident = &vf.ident;
      quote! { #ident: f(&Self::PARAMS[#index], self.#ident) }
    })
    .collect();
  let zipped: Vec<_> = voice_fields
    .iter()
    .enumerate()
    .map(|(index, vf)| {
      let ident = &vf.ident;
      quote! {
        #ident: f(&Self::PARAMS[#index], self.#ident, other.#ident)
      }
    })
    .collect();

  quote! {
    impl #name {
      /// Every parameter's value, in `PARAMS` order.
      pub fn values(&self) -> [f64; #count] {
        [#(#values,)*]
      }

      /// This patch with every parameter replaced by `f(meta, value)`.
      pub fn map_params(
        &self,
        mut f: impl FnMut(&'static PatchParamMeta, f64) -> f64,
      ) -> Self {
        Self { #(#mapped,)* }
      }

      /// A patch whose every parameter is `f(meta, self's value, other's
      /// value)`.
      pub fn zip_params(
        &self,
        other: &Self,
        mut f: impl FnMut(&'static PatchParamMeta, f64, f64) -> f64,
      ) -> Self {
        Self { #(#zipped,)* }
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
