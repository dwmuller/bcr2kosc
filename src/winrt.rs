use super::{LocalError, Result};
use registry::*;
use simple_error::bail;
use utfx::U16CString;

#[derive(Clone)]
pub enum PortType {
    Input,
    Output,
}
pub fn rename_port(ptype: &PortType, name: &str, new_name: &str) -> Result<()> {
    let regkey = Hive::LocalMachine.open(
        match ptype {
            PortType::Input => r"SYSTEM\CurrentControlSet\Control\DeviceClasses\{504be32c-ccf6-4d2c-b73f-6f8b3747e22b}",
            PortType::Output => r"SYSTEM\CurrentControlSet\Control\DeviceClasses\{6dc23320-ab33-4ce4-80d4-bbb3ebbf2814}",
        },
        Security::Read,
    ).expect("registry device class should exist");
    let name = U16CString::from_str(name).expect("port name should be compatible with UTF-16");
    for k in regkey.keys() {
        let k = k?
            .open(Security::Read)?
            .open(
                r"#\Device Parameters",
                Security::QueryValue | Security::SetValue,
            )
            .expect("process should have value query & set access, please run as admin");
        if let Data::String(s) = k.value("FriendlyName")? {
            if s == name {
                let new_name = Data::String(
                    U16CString::from_str(new_name)
                        .expect("new port name should be compatible with UTF-16"),
                );
                k.set_value("FriendlyName", &new_name)
                    .expect("expected to update FriendlyName");
                return Ok(());
            }
        }
    }
    bail!("Port not found.")
}

pub fn parse_port_type_arg(s: &str) -> Result<PortType> {
    match s {
        "input" => Ok(PortType::Input),
        "output" => Ok(PortType::Output),
        _ => Err(LocalError::from("invalid port type")),
    }
}
