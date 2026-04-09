use syn::{Field, Lit};

/// A parsed `#[patch_param(min = X, max = Y, step = Z, ...)]` field.
pub struct PatchField {
  pub ident: syn::Ident,
  /// UI slider minimum.
  pub min: f64,
  /// UI slider maximum.
  pub max: f64,
  /// UI slider step size (defaults to 0.01 when absent).
  pub step: f64,
  /// Human-readable description for the UI.
  pub description: String,
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

    let attr = match attr {
      Some(a) => a,
      None => return Ok(None),
    };

    // Validate that the field type is f64.
    if !is_f64(&field.ty) {
      return Err(syn::Error::new_spanned(
        &field.ty,
        "patch_param fields must be f64",
      ));
    }

    let mut min: Option<f64> = None;
    let mut max: Option<f64> = None;
    let mut step: Option<f64> = None;
    let mut description: Option<String> = None;

    attr.parse_nested_meta(|meta| {
      if meta.path.is_ident("min") {
        let value = meta.value()?;
        let lit: Lit = value.parse()?;
        min = Some(parse_float_lit(&lit, &meta)?);
        Ok(())
      } else if meta.path.is_ident("max") {
        let value = meta.value()?;
        let lit: Lit = value.parse()?;
        max = Some(parse_float_lit(&lit, &meta)?);
        Ok(())
      } else if meta.path.is_ident("step") {
        let value = meta.value()?;
        let lit: Lit = value.parse()?;
        step = Some(parse_float_lit(&lit, &meta)?);
        Ok(())
      } else if meta.path.is_ident("description") {
        let value = meta.value()?;
        let lit: Lit = value.parse()?;
        match &lit {
          Lit::Str(s) => {
            description = Some(s.value());
            Ok(())
          }
          _ => Err(meta.error("description must be a string")),
        }
      } else {
        Err(meta.error("expected `min`, `max`, `step`, or `description`"))
      }
    })?;

    let min =
      min.ok_or_else(|| syn::Error::new_spanned(attr, "missing `min`"))?;
    let max =
      max.ok_or_else(|| syn::Error::new_spanned(attr, "missing `max`"))?;

    Ok(Some(PatchField {
      ident,
      min,
      max,
      step: step.unwrap_or(0.01),
      description: description.unwrap_or_default(),
    }))
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
