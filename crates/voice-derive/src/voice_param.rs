use syn::{Expr, ExprLit, ExprRange, Field, Lit};

/// A parsed `#[voice_param(order = N, range = LO..HI)]` field.
pub struct VoiceField {
  pub ident: syn::Ident,
  pub order: u32,
  pub range_lo: f64,
  pub range_hi: f64,
}

impl VoiceField {
  /// Parse a struct field's attributes.  Returns `None` if
  /// the field has no `voice_param` attribute.
  pub fn from_field(field: &Field) -> syn::Result<Option<Self>> {
    let ident = field.ident.clone().ok_or_else(|| {
      syn::Error::new_spanned(field, "voice_param requires named fields")
    })?;

    let attr = field
      .attrs
      .iter()
      .find(|a| a.path().is_ident("voice_param"));

    let attr = match attr {
      Some(a) => a,
      None => return Ok(None),
    };

    // Validate that the field type is f64.
    if !is_f64(&field.ty) {
      return Err(syn::Error::new_spanned(
        &field.ty,
        "voice_param fields must be f64",
      ));
    }

    let mut order: Option<u32> = None;
    let mut range_lo: Option<f64> = None;
    let mut range_hi: Option<f64> = None;

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
      } else {
        Err(meta.error("expected `order` or `range`"))
      }
    })?;

    let order =
      order.ok_or_else(|| syn::Error::new_spanned(attr, "missing `order`"))?;
    let range_lo = range_lo
      .ok_or_else(|| syn::Error::new_spanned(attr, "missing range start"))?;
    let range_hi = range_hi
      .ok_or_else(|| syn::Error::new_spanned(attr, "missing range end"))?;

    Ok(Some(VoiceField {
      ident,
      order,
      range_lo,
      range_hi,
    }))
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
