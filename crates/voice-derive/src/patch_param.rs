use syn::{Expr, ExprLit, ExprRange, ExprUnary, Field, Lit, UnOp};

/// A parsed `#[patch_param(order = N, range = LO..HI, ...)]` field.
pub struct PatchField {
  pub ident: syn::Ident,
  pub order: u32,
  pub range_lo: f64,
  pub range_hi: f64,
  /// UI slider minimum (falls back to range_lo when absent).
  pub min: Option<f64>,
  /// UI slider maximum (falls back to range_hi when absent).
  pub max: Option<f64>,
  /// UI slider step size (defaults to 0.01 when absent).
  pub step: Option<f64>,
  /// Human-readable description for the UI.
  pub description: Option<String>,
}

impl PatchField {
  /// Effective UI minimum (min if set, otherwise range_lo).
  pub fn effective_min(&self) -> f64 {
    self.min.unwrap_or(self.range_lo)
  }

  /// Effective UI maximum (max if set, otherwise range_hi).
  pub fn effective_max(&self) -> f64 {
    self.max.unwrap_or(self.range_hi)
  }

  /// Effective UI step size.
  pub fn effective_step(&self) -> f64 {
    self.step.unwrap_or(0.01)
  }

  /// Effective description (empty string when absent).
  pub fn effective_description(&self) -> String {
    self.description.clone().unwrap_or_default()
  }

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

    let mut order: Option<u32> = None;
    let mut range_lo: Option<f64> = None;
    let mut range_hi: Option<f64> = None;
    let mut min: Option<f64> = None;
    let mut max: Option<f64> = None;
    let mut step: Option<f64> = None;
    let mut description: Option<String> = None;

    attr.parse_nested_meta(|meta| {
      if meta.path.is_ident("order") {
        let value = meta.value()?;
        let lit: Lit = value.parse()?;
        match &lit {
          Lit::Int(i) => {
            order = Some(i.base10_parse()?);
            Ok(())
          }
          _ => Err(meta.error("order must be an integer")),
        }
      } else if meta.path.is_ident("range") {
        let value = meta.value()?;
        let range: ExprRange = value.parse()?;
        range_lo = extract_float(range.start.as_deref())?;
        range_hi = extract_float(range.end.as_deref())?;
        Ok(())
      } else if meta.path.is_ident("min") {
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
        Err(meta.error(
          "expected `order`, `range`, `min`, `max`, `step`, \
           or `description`",
        ))
      }
    })?;

    let order =
      order.ok_or_else(|| syn::Error::new_spanned(attr, "missing `order`"))?;
    let range_lo = range_lo
      .ok_or_else(|| syn::Error::new_spanned(attr, "missing range start"))?;
    let range_hi = range_hi
      .ok_or_else(|| syn::Error::new_spanned(attr, "missing range end"))?;

    Ok(Some(PatchField {
      ident,
      order,
      range_lo,
      range_hi,
      min,
      max,
      step,
      description,
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

fn extract_float(expr: Option<&Expr>) -> syn::Result<Option<f64>> {
  match expr {
    None => Ok(None),
    Some(Expr::Lit(ExprLit {
      lit: Lit::Float(f), ..
    })) => Ok(Some(f.base10_parse()?)),
    Some(Expr::Lit(ExprLit {
      lit: Lit::Int(i), ..
    })) => Ok(Some(i.base10_parse::<f64>()?)),
    Some(Expr::Unary(ExprUnary {
      op: UnOp::Neg(_),
      expr: inner,
      ..
    })) => {
      let positive = extract_float(Some(inner))?;
      Ok(positive.map(|v| -v))
    }
    Some(other) => {
      Err(syn::Error::new_spanned(other, "expected a float literal"))
    }
  }
}

fn is_f64(ty: &syn::Type) -> bool {
  match ty {
    syn::Type::Path(p) => p.path.is_ident("f64"),
    _ => false,
  }
}
