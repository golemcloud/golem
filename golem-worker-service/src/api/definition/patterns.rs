#[derive(Debug)]
pub struct VarInfo {
    pub key_name: String,
}

#[derive(Debug)]
pub struct CatchAllVarInfo {
    pub key_name: String,
}

#[derive(Debug)]
pub enum PathPattern {
    Literal(String),
    Var(VarInfo),
    CatchAllVar(CatchAllVarInfo),
}

#[derive(Debug)]
pub struct AllPathPatterns {
    pub path_patterns: Vec<PathPattern>,
}

impl AllPathPatterns {
    pub fn parse(path: &str) -> Result<Self, String> {
        let mut patterns = Vec::new();
        for segment in path.split('/') {
            if segment.is_empty() {
                continue;
            }

            if segment.starts_with('{') && segment.ends_with('}') {
                let key_name = segment[1..segment.len()-1].to_string();
                if key_name.ends_with("...") {
                    patterns.push(PathPattern::CatchAllVar(CatchAllVarInfo {
                        key_name: key_name[..key_name.len()-3].to_string(),
                    }));
                } else {
                    patterns.push(PathPattern::Var(VarInfo {
                        key_name,
                    }));
                }
            } else {
                patterns.push(PathPattern::Literal(segment.to_string()));
            }
        }
        Ok(Self { path_patterns: patterns })
    }
}
