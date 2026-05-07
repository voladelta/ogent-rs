#[derive(Debug, Clone)]
pub struct Profile {
  pub backend: &'static str,
  pub model: &'static str,
  pub effort: &'static str,
  pub context_limit: usize,
}

pub fn get_profile(name: &str) -> Option<Profile> {
  Some(match name {
    "ds-flash" => Profile {
      backend: "deepseek",
      model: "deepseek-v4-flash",
      effort: "high",
      context_limit: 1_000_000,
    },
    "ds-flash-max" => Profile {
      backend: "deepseek",
      model: "deepseek-v4-flash",
      effort: "max",
      context_limit: 1_000_000,
    },
    "ds-pro" => Profile {
      backend: "deepseek",
      model: "deepseek-v4-pro",
      effort: "high",
      context_limit: 1_000_000,
    },
    "ds-pro-max" => Profile {
      backend: "deepseek",
      model: "deepseek-v4-pro",
      effort: "max",
      context_limit: 1_000_000,
    },
    "kimi" => Profile {
      backend: "kimi",
      model: "moonshotai/Kimi-K2.6",
      effort: "",
      context_limit: 256_000,
    },
    "glm" => Profile {
      backend: "z",
      model: "glm-5.1",
      effort: "",
      context_limit: 200_000,
    },
    _ => return None,
  })
}
