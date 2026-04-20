use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct InterfaceConfig {
    pub name: String,
    pub iface_type: InterfaceType,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub enum InterfaceType {
    TcpClient {
        target_host: String,
        target_port: u16,
    },
    TcpServer {
        listen_port: u16,
    },
    Udp {
        bind_addr: String,
    },
}

#[derive(Debug, Default)]
pub struct RnsConfig {
    pub interfaces: Vec<InterfaceConfig>,
    pub enable_transport: bool,
}

impl RnsConfig {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| ConfigError::Io(path.as_ref().display().to_string(), e))?;
        Self::parse(&content)
    }

    pub fn parse(content: &str) -> Result<Self, ConfigError> {
        let mut config = RnsConfig::default();
        let mut current_section: Option<String> = None;
        let mut current_interface: Option<String> = None;
        let mut interface_props: HashMap<String, String> = HashMap::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                if let Some(iface_name) = current_interface.take() {
                    if let Ok(iface) = parse_interface(&iface_name, &interface_props) {
                        config.interfaces.push(iface);
                    }
                    interface_props.clear();
                }
                let section = line[1..line.len() - 1].trim();
                current_section = Some(section.to_string());
                if section.starts_with('[') && section.ends_with(']') {
                    let iface_name = section[1..section.len() - 1].trim().to_string();
                    current_interface = Some(iface_name);
                } else if section == "interfaces" {
                    current_interface = None;
                }
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                if current_interface.is_some() {
                    interface_props.insert(key.to_string(), value.to_string());
                    continue;
                }
                if current_section.as_deref() == Some("reticulum") && key == "enable_transport" {
                    config.enable_transport = parse_bool(value);
                }
            }
        }
        if let Some(iface_name) = current_interface {
            if let Ok(iface) = parse_interface(&iface_name, &interface_props) {
                config.interfaces.push(iface);
            }
        }
        Ok(config)
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(value.to_lowercase().as_str(), "true" | "yes" | "1" | "on")
}

fn parse_interface(
    name: &str,
    props: &HashMap<String, String>,
) -> Result<InterfaceConfig, ConfigError> {
    let iface_type_str = props
        .get("type")
        .or_else(|| props.get("interface_type"))
        .ok_or_else(|| ConfigError::MissingField(name.to_string(), "type"))?;

    let enabled = props
        .get("interface_enabled")
        .or_else(|| props.get("enabled"))
        .map(|v| parse_bool(v))
        .unwrap_or(true);

    let iface_type = match iface_type_str.as_str() {
        "TCPClientInterface" => {
            let target_host = props
                .get("target_host")
                .ok_or_else(|| ConfigError::MissingField(name.to_string(), "target_host"))?
                .to_string();
            let target_port = props
                .get("target_port")
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| ConfigError::MissingField(name.to_string(), "target_port"))?;
            InterfaceType::TcpClient {
                target_host,
                target_port,
            }
        }
        "TCPServerInterface" => {
            let listen_port = props
                .get("listen_port")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4242);
            InterfaceType::TcpServer { listen_port }
        }
        "UDPInterface" => {
            let bind_addr = props
                .get("listen_addr")
                .or_else(|| props.get("bind_addr"))
                .map(|v| v.to_string())
                .unwrap_or_else(|| "0.0.0.0:4242".to_string());
            InterfaceType::Udp { bind_addr }
        }
        _ => {
            return Err(ConfigError::UnknownInterfaceType(
                name.to_string(),
                iface_type_str.clone(),
            ));
        }
    };

    Ok(InterfaceConfig {
        name: name.to_string(),
        iface_type,
        enabled,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error reading {0}: {1}")]
    Io(String, std::io::Error),
    #[error("Missing field {1} in interface {0}")]
    MissingField(String, &'static str),
    #[error("Unknown interface type {1} in interface {0}")]
    UnknownInterfaceType(String, String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tcp_client_interface() {
        let config = r#"
[reticulum]

[[RNS Testnet Amsterdam]]
  type = TCPClientInterface
  target_host = amsterdam.reticulum.network
  target_port = 4985
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert_eq!(parsed.interfaces.len(), 1);
        let iface = &parsed.interfaces[0];
        assert_eq!(iface.name, "RNS Testnet Amsterdam");
        assert!(iface.enabled);
        match &iface.iface_type {
            InterfaceType::TcpClient {
                target_host,
                target_port,
            } => {
                assert_eq!(target_host, "amsterdam.reticulum.network");
                assert_eq!(*target_port, 4985);
            }
            _ => panic!("expected TcpClient"),
        }
    }

    #[test]
    fn parse_tcp_server_with_custom_port() {
        let config = r#"
[[My Server]]
  type = TCPServerInterface
  listen_port = 4321
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert_eq!(parsed.interfaces.len(), 1);
        match &parsed.interfaces[0].iface_type {
            InterfaceType::TcpServer { listen_port } => assert_eq!(*listen_port, 4321),
            _ => panic!("expected TcpServer"),
        }
    }

    #[test]
    fn parse_udp_interface() {
        let config = r#"
[[Local UDP]]
  type = UDPInterface
  listen_addr = 0.0.0.0:4242
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert_eq!(parsed.interfaces.len(), 1);
        match &parsed.interfaces[0].iface_type {
            InterfaceType::Udp { bind_addr } => assert_eq!(bind_addr, "0.0.0.0:4242"),
            _ => panic!("expected UDP"),
        }
    }

    #[test]
    fn disabled_interface_skipped() {
        let config = r#"
[[Disabled Iface]]
  type = TCPClientInterface
  target_host = example.com
  target_port = 1234
  interface_enabled = no
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert_eq!(parsed.interfaces.len(), 1);
        assert!(!parsed.interfaces[0].enabled);
    }

    #[test]
    fn enable_transport_parsed() {
        let config = r#"
[reticulum]
  enable_transport = true
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert!(parsed.enable_transport);
    }

    #[test]
    fn comments_and_empty_lines_ignored() {
        let config = r#"
# This is a comment
  # Indented comment

[[Test]]
  type = UDPInterface
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert_eq!(parsed.interfaces.len(), 1);
    }

    #[test]
    fn unknown_type_is_skipped() {
        let config = r#"
[[Bad]]
  type = FakeInterface
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert_eq!(parsed.interfaces.len(), 0);
    }

    #[test]
    fn missing_type_is_skipped() {
        let config = r#"
[[NoType]]
  target_host = example.com
"#;
        let parsed = RnsConfig::parse(config).unwrap();
        assert_eq!(parsed.interfaces.len(), 0);
    }
}
