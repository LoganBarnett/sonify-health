use sonify_health_voice_derive::VoiceGenerate;

#[derive(VoiceGenerate)]
struct Bad {
  #[voice_param(order = 0, range = 0.0..1.0)]
  a: f64,
  #[voice_param(order = 2, range = 0.0..1.0)]
  b: f64,
}

fn main() {}
