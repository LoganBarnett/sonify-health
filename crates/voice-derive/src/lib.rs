//! Derive macro for deterministic patch parameter generation.
//!
//! `#[derive(PatchGenerate)]` on a struct with
//! `#[patch_param(order = N, range = LO..HI)]` annotations
//! generates a `from_hostname(&str) -> Self` constructor that
//! seeds a PRNG from the hostname hash and draws each field in
//! the declared order.
//!
//! Also generates:
//! - `PatchOverrides` struct with `Option<f64>` mirrors
//! - `with_overrides()` method
//! - `PARAMS` constant (`&'static [PatchParamMeta]`)
//! - `set_param()` / `get_param()` methods
//! - `PatchOverrides::to_fields()` method
//!
//! Compile-time checks:
//! - Every `patch_param` field must be `f64`.
//! - `order` values must form a contiguous 0..N sequence.
//! - No duplicate `order` values.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

mod patch_param;
use patch_param::PatchField;

/// Derive `from_hostname` for deterministic voice generation.
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
  let mut plain_fields: Vec<syn::Ident> = Vec::new();

  for field in fields {
    match PatchField::from_field(field)? {
      Some(vf) => voice_fields.push(vf),
      None => {
        if let Some(ident) = &field.ident {
          plain_fields.push(ident.clone());
        }
      }
    }
  }

  validate_orders(&voice_fields)?;

  // Sort by order for deterministic draw sequence.
  voice_fields.sort_by_key(|f| f.order);

  let from_hostname = gen_from_hostname(name, &voice_fields, &plain_fields);
  let params_const = gen_params_const(name, &voice_fields);
  let set_get_param = gen_set_get_param(name, &voice_fields);
  let with_overrides = gen_with_overrides(name, &voice_fields);
  let overrides_struct = gen_overrides_struct(&voice_fields);

  Ok(quote! {
    #from_hostname
    #params_const
    #set_get_param
    #with_overrides
    #overrides_struct
  })
}

fn gen_from_hostname(
  name: &syn::Ident,
  voice_fields: &[PatchField],
  plain_fields: &[syn::Ident],
) -> proc_macro2::TokenStream {
  let draw_stmts: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      let lo = vf.range_lo;
      let hi = vf.range_hi;
      quote! {
        let #ident: f64 = rng.gen_range(#lo..#hi);
      }
    })
    .collect();

  let field_inits: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let ident = &vf.ident;
      quote! { #ident }
    })
    .collect();

  let default_inits: Vec<_> = plain_fields
    .iter()
    .map(|ident| {
      quote! { #ident: Default::default() }
    })
    .collect();

  quote! {
    impl #name {
      /// Derive parameters from a hostname using a
      /// deterministic PRNG.  Draw order is fixed by the
      /// `order` attribute — appending new fields is safe,
      /// but reordering existing ones changes all voices.
      pub fn from_hostname(hostname: &str) -> Self {
        use ::sha2::Digest;
        use ::rand::Rng;
        use ::rand::SeedableRng;

        let hash = ::sha2::Sha256::digest(
          hostname.as_bytes(),
        );
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&hash);
        let mut rng =
          ::rand_xoshiro::Xoshiro256StarStar::from_seed(
            seed,
          );

        #(#draw_stmts)*

        Self {
          #(#field_inits,)*
          #(#default_inits,)*
        }
      }
    }
  }
}

fn gen_params_const(
  name: &syn::Ident,
  voice_fields: &[PatchField],
) -> proc_macro2::TokenStream {
  let entries: Vec<_> = voice_fields
    .iter()
    .map(|vf| {
      let field_name = vf.ident.to_string();
      let min = vf.effective_min();
      let max = vf.effective_max();
      let step = vf.effective_step();
      let desc = vf.effective_description();
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

fn validate_orders(fields: &[PatchField]) -> syn::Result<()> {
  // Check for duplicate orders.
  let mut seen = std::collections::HashMap::<u32, &syn::Ident>::new();
  for vf in fields {
    if let Some(prev) = seen.get(&vf.order) {
      return Err(syn::Error::new_spanned(
        &vf.ident,
        format!(
          "duplicate patch_param order {}: already used \
           by `{}`",
          vf.order, prev
        ),
      ));
    }
    seen.insert(vf.order, &vf.ident);
  }

  // Check for contiguous 0..N.
  let max = fields.len() as u32;
  for i in 0..max {
    if !seen.contains_key(&i) {
      return Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        format!(
          "patch_param order {} is missing — orders must \
           be contiguous 0..{}",
          i, max
        ),
      ));
    }
  }

  Ok(())
}
