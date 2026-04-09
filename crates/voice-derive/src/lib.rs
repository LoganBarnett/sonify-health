//! Derive macro for deterministic patch parameter generation.
//!
//! `#[derive(PatchGenerate)]` on a struct with
//! `#[patch_param(order = N, range = LO..HI)]` annotations
//! generates a `from_hostname(&str) -> Self` constructor that
//! seeds a PRNG from the hostname hash and draws each field in
//! the declared order.
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

  let expanded = quote! {
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
  };

  Ok(expanded)
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
