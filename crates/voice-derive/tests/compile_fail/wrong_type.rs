use sonify_health_voice_derive::PatchGenerate;

#[derive(PatchGenerate)]
struct Bad {
  #[patch_param(min = 0.0, max = 1.0)]
  a: f32,
}

fn main() {}
