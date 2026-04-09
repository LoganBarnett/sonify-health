use sonify_health_voice_derive::PatchGenerate;

#[derive(PatchGenerate)]
struct Bad {
  #[patch_param(order = 0, range = 0.0..1.0)]
  a: f32,
}

fn main() {}
