use crate::contract::ir::HostRef;

pub(crate) fn global(host: &HostRef) -> &str {
    let HostRef::Global(name) = host else {
        unreachable!("global binding")
    };
    name
}

pub(crate) fn import(host: &HostRef) -> &str {
    let HostRef::Import { module, name } = host else {
        unreachable!("import binding")
    };
    super::host_modules::symbol(module, name)
}

pub(crate) fn symbol(host: &HostRef) -> &str {
    match host {
        HostRef::Global(_) => global(host),
        HostRef::Import { .. } => import(host),
    }
}
