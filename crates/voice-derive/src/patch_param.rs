use syn::{Field, Lit};

/// A parsed `#[patch_param(min = X, max = Y, step = Z, default = D, ...)]`
/// field.
///
/// A parameter carries two tiers of bounds.  `min` and `max` are the
/// dashboard slider's range, chosen to taste: a value past them is honoured.
/// `limit_min`, `limit_max`, and `positive` are hard limits, declared only
/// where a value past them is meaningless or a blatant error.
pub struct PatchField {
  pub ident: syn::Ident,
  /// UI slider minimum.
  pub min: f64,
  /// UI slider maximum.
  pub max: f64,
  /// UI slider step size (defaults to 0.01 when absent).
  pub step: f64,
  /// UI slider maps position to value logarithmically.  Requires a
  /// positive `min`.
  pub logarithmic: bool,
  /// Value of a freshly built patch, and what a meaningless value reads as.
  pub default: f64,
  /// Hard lower limit, inclusive; `None` is unbounded.
  pub limit_min: Option<f64>,
  /// Hard upper limit, inclusive; `None` is unbounded.
  pub limit_max: Option<f64>,
  /// Only strictly positive values work.  Zero has no nearest valid value
  /// to clamp to, so a value at or below zero reads as `default`.
  pub positive: bool,
  /// Human-readable description for the UI.
  pub description: String,
}

/// Values read from one `patch_param` attribute, before validation.
#[derive(Default)]
struct RawAttribute {
  min: Option<f64>,
  max: Option<f64>,
  step: Option<f64>,
  logarithmic: bool,
  default: Option<f64>,
  limit_min: Option<f64>,
  limit_max: Option<f64>,
  positive: bool,
  description: Option<String>,
}

impl PatchField {
  /// Parse a struct field's attributes.  Returns `None` if
  /// the field has no `patch_param` attribute.
  pub fn from_field(field: &Field) -> syn::Result<Option<Self>> {
    let ident = field.ident.clone().ok_or_else(|| {
      syn::Error::new_spanned(field, "patch_param requires named fields")
    })?;

    let attr = field
      .attrs
      .iter()
      .find(|a| a.path().is_ident("patch_param"));

    let Some(attr) = attr else {
      return Ok(None);
    };

    // Validate that the field type is f64.
    if !is_f64(&field.ty) {
      return Err(syn::Error::new_spanned(
        &field.ty,
        "patch_param fields must be f64",
      ));
    }

    let mut raw = RawAttribute::default();
    attr.parse_nested_meta(|meta| {
      if meta.path.is_ident("logarithmic") {
        raw.logarithmic = true;
        Ok(())
      } else if meta.path.is_ident("positive") {
        raw.positive = true;
        Ok(())
      } else if meta.path.is_ident("description") {
        let lit: Lit = meta.value()?.parse()?;
        match &lit {
          Lit::Str(s) => {
            raw.description = Some(s.value());
            Ok(())
          }
          _ => Err(meta.error("description must be a string")),
        }
      } else {
        let slot = if meta.path.is_ident("min") {
          Some(&mut raw.min)
        } else if meta.path.is_ident("max") {
          Some(&mut raw.max)
        } else if meta.path.is_ident("step") {
          Some(&mut raw.step)
        } else if meta.path.is_ident("default") {
          Some(&mut raw.default)
        } else if meta.path.is_ident("limit_min") {
          Some(&mut raw.limit_min)
        } else if meta.path.is_ident("limit_max") {
          Some(&mut raw.limit_max)
        } else {
          None
        }
        .ok_or_else(|| {
          meta.error(
            "expected `min`, `max`, `step`, `default`, `limit_min`, \
             `limit_max`, `positive`, `logarithmic`, or `description`",
          )
        })?;
        let lit: Lit = meta.value()?.parse()?;
        *slot = Some(parse_float_lit(&lit, &meta)?);
        Ok(())
      }
    })?;

    let missing =
      |key: &str| syn::Error::new_spanned(attr, format!("missing `{key}`"));
    let field = PatchField {
      ident,
      min: raw.min.ok_or_else(|| missing("min"))?,
      max: raw.max.ok_or_else(|| missing("max"))?,
      step: raw.step.unwrap_or(0.01),
      logarithmic: raw.logarithmic,
      default: raw.default.ok_or_else(|| missing("default"))?,
      limit_min: raw.limit_min,
      limit_max: raw.limit_max,
      positive: raw.positive,
      description: raw.description.unwrap_or_default(),
    };
    field
      .broken_rule()
      .map_or(Ok(Some(field)), |rule| Err(syn::Error::new_spanned(attr, rule)))
  }

  /// The first rule the declared bounds break, if any.  Each one is a
  /// mistake in the declaration rather than a matter of taste, so it is a
  /// compile error.
  fn broken_rule(&self) -> Option<&'static str> {
    let rules = [
      (
        [self.min, self.max, self.step, self.default]
          .into_iter()
          .chain(self.limit_min)
          .chain(self.limit_max)
          .any(|v| !v.is_finite()),
        "every bound and the default must be a finite number",
      ),
      (self.min >= self.max, "`min` must be below `max`"),
      (self.step <= 0.0, "`step` must be positive"),
      (
        self.limit_min.is_some_and(|lo| lo > self.min),
        "`limit_min` must not exceed `min`: a hard limit contains the \
         slider's range",
      ),
      (
        self.limit_max.is_some_and(|hi| hi < self.max),
        "`limit_max` must not fall below `max`: a hard limit contains the \
         slider's range",
      ),
      (
        self.positive && self.limit_min.is_some(),
        "`positive` already bounds the parameter below, so it excludes \
         `limit_min`",
      ),
      (
        self.positive && self.min <= 0.0,
        "`positive` requires a positive `min`",
      ),
      (
        self.default < self.min || self.default > self.max,
        "`default` must lie within `min..=max`",
      ),
      (
        self.logarithmic && self.min <= 0.0,
        "`logarithmic` requires a positive `min`",
      ),
    ];
    rules
      .into_iter()
      .find(|(broken, _)| *broken)
      .map(|(_, rule)| rule)
  }
}

fn parse_float_lit(
  lit: &Lit,
  meta: &syn::meta::ParseNestedMeta<'_>,
) -> syn::Result<f64> {
  match lit {
    Lit::Float(f) => f.base10_parse(),
    Lit::Int(i) => i.base10_parse::<f64>(),
    _ => Err(meta.error("expected a numeric literal")),
  }
}

fn is_f64(ty: &syn::Type) -> bool {
  match ty {
    syn::Type::Path(p) => p.path.is_ident("f64"),
    _ => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use syn::parse_quote;

  /// The parse result for a field carrying `attribute`.
  fn parse(attribute: proc_macro2::TokenStream) -> syn::Result<PatchField> {
    let input: syn::DeriveInput = parse_quote! {
      struct S {
        #[patch_param(#attribute)]
        x: f64,
      }
    };
    let syn::Data::Struct(data) = input.data else {
      panic!("the test input is a struct");
    };
    let field = data.fields.iter().next().expect("one field");
    PatchField::from_field(field)
      .map(|parsed| parsed.expect("the field has a patch_param attribute"))
  }

  fn rejection(attribute: proc_macro2::TokenStream) -> String {
    match parse(attribute) {
      Ok(_) => panic!("the attribute was accepted"),
      Err(error) => error.to_string(),
    }
  }

  #[test]
  fn accepts_a_full_declaration() {
    let field = parse(quote::quote! {
      min = 0.1, max = 10.0, step = 0.01, default = 1.0, positive,
      logarithmic, description = "Width."
    })
    .unwrap();
    assert!(field.positive && field.logarithmic);
    assert_eq!(field.default, 1.0);
    assert_eq!((field.limit_min, field.limit_max), (None, None));
    let limited = parse(quote::quote! {
      min = -1.0, max = 1.0, default = 0.0, limit_min = -1.0, limit_max = 1.0
    })
    .unwrap();
    assert_eq!((limited.limit_min, limited.limit_max), (Some(-1.0), Some(1.0)));
  }

  #[test]
  fn requires_a_default() {
    assert!(rejection(quote::quote! { min = 0.0, max = 1.0 })
      .contains("missing `default`"));
  }

  #[test]
  fn rejects_an_inverted_slider() {
    assert!(rejection(quote::quote! { min = 1.0, max = 0.0, default = 0.5 })
      .contains("`min` must be below `max`"));
  }

  #[test]
  fn rejects_a_non_positive_step() {
    assert!(rejection(quote::quote! {
      min = 0.0, max = 1.0, step = 0.0, default = 0.5
    })
    .contains("`step` must be positive"));
  }

  #[test]
  fn rejects_a_hard_limit_inside_the_slider() {
    assert!(rejection(quote::quote! {
      min = 0.0, max = 1.0, default = 0.5, limit_min = 0.2
    })
    .contains("`limit_min` must not exceed `min`"));
    assert!(rejection(quote::quote! {
      min = 0.0, max = 1.0, default = 0.5, limit_max = 0.8
    })
    .contains("`limit_max` must not fall below `max`"));
  }

  #[test]
  fn rejects_positive_with_a_lower_limit_or_a_slider_at_zero() {
    assert!(rejection(quote::quote! {
      min = 0.1, max = 1.0, default = 0.5, positive, limit_min = 0.0
    })
    .contains("excludes `limit_min`"));
    assert!(rejection(quote::quote! {
      min = 0.0, max = 1.0, default = 0.5, positive
    })
    .contains("`positive` requires a positive `min`"));
  }

  #[test]
  fn rejects_a_default_outside_the_slider() {
    assert!(rejection(quote::quote! { min = 0.0, max = 1.0, default = 2.0 })
      .contains("`default` must lie within"));
  }

  #[test]
  fn rejects_a_logarithmic_slider_reaching_zero() {
    assert!(rejection(quote::quote! {
      min = 0.0, max = 1.0, default = 0.5, logarithmic
    })
    .contains("`logarithmic` requires a positive `min`"));
  }

  #[test]
  fn rejects_a_non_finite_literal() {
    assert!(rejection(quote::quote! {
      min = 0.0, max = 1.0, default = 0.5, limit_max = 1e400
    })
    .contains("finite"));
  }
}
