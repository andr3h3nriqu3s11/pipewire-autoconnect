use std::{cell::RefCell, env, fs, io::BufRead, collections::HashMap};
use std::vec::Vec;
use std::{cell::RefMut, rc::Rc};

use libspa::ReadableDict;
use pipewire::{types::ObjectType, Context, MainLoop};
use pipewire as pw;
use regex::Regex;

#[macro_use]
extern crate lazy_static;

#[derive(Debug)]
struct Port {
    id: u32,
    name: String,
    node: Rc<Node>,
}

#[derive(Debug)]
struct Node {
    id: u32,
    name: String,
}

#[derive(Debug)]
struct NodeDef {
    name: String,
}

#[derive(Debug)]
struct PortDef {
    node: Rc<NodeDef>,
    name: String,
}

#[derive(Debug)]
struct LinkDef {
    port_in: Rc<PortDef>,
    port_out: Rc<PortDef>,
}

struct AppState {
    ports: Vec<Rc<Port>>,
    nodes: Vec<Rc<Node>>,

    get_names: bool,

    node_def: Vec<Rc<NodeDef>>,
    link_def: Vec<Rc<LinkDef>>,
    port_def: Vec<Rc<PortDef>>,
}

fn search<T, P>(v: &[Rc<T>], f: P) -> Option<Rc<T>>
where
    T: Sized,
    P: FnMut(&&Rc<T>) -> bool,
{
    let n = v.iter().filter(f).cloned().collect::<Vec<Rc<T>>>();

    if n.len() != 1 {
        return None;
    }

    Some(n[0].clone())
}

impl AppState {
    fn new(
        node_def: Vec<Rc<NodeDef>>,
        link_def: Vec<Rc<LinkDef>>,
        port_def: Vec<Rc<PortDef>>,
        get_names: bool,
    ) -> AppState {
        AppState {
            node_def,
            link_def,
            port_def,
            get_names,
            ports: Vec::new(),
            nodes: Vec::new(),
        }
    }

    fn try_add_node(&mut self, def: Node) -> bool {
        if !self.node_def.iter().any(|a| a.name.eq(&def.name)) {
            return false;
        };

        let mut nodes = self
            .nodes
            .iter()
            .filter(|a| !a.name.eq(&def.name))
            .cloned()
            .collect::<Vec<Rc<Node>>>();

        nodes.push(Rc::new(def));

        self.nodes = nodes;

        true
    }

    fn get_node(&self, id: u32) -> Option<Rc<Node>> {
        search(&self.nodes, |a| a.id == id)
    }

    fn get_port_by_name(&self, name: String) -> Option<Rc<Port>> {
        search(&self.ports, |a| a.name.eq(&name))
    }

    fn try_add_port(&mut self, id: u32, name: String, node_id: u32) -> bool {
        let node = self.get_node(node_id);

        if node.is_none() {
            return false;
        }

        let node = node.unwrap();

        if self
            .port_def
            .iter()
            .filter(|a| a.name.eq(&name) && a.node.name.eq(&node.name))
            .count()
            != 1
        {
            if self.get_names && node.id == node_id {
                println!("Port from node {} -> {}: {}", &node.name, id, name);
            }
            return false;
        }

        let mut ports = self
            .ports
            .iter()
            .filter(|a| !(a.name.eq(&name) && a.node.name.eq(&node.name)))
            .cloned()
            .collect::<Vec<Rc<Port>>>();

        ports.push(Rc::new(Port { id, name, node }));

        self.ports = ports;

        true
    }

    fn create_links(&mut self, port_name: String, core: Rc<pw::Core>) {
        #[derive(Debug)]
        struct TempLink {
            port_in: Option<Rc<Port>>,
            port_out: Option<Rc<Port>>,
        }

        self.link_def
            .iter()
            .filter(|link| (link.port_in.name.eq(&port_name) || link.port_out.name.eq(&port_name)))
            .map(|a| TempLink {
                port_in: self.get_port_by_name(a.port_in.name.to_string()),
                port_out: self.get_port_by_name(a.port_out.name.to_string()),
            }).filter(|a| a.port_in.is_some() && a.port_out.is_some()).for_each(|a| {
                let port_in = a.port_in.unwrap();
                let port_out = a.port_out.unwrap();

                println!("Try to created link: [{}]{} -> [{}]{}", port_out.node.name, port_out.name, port_in.node.name, port_in.name);
                
                // Try to create the link
                if core.create_object::<pw::link::Link, _>(
                    // The actual name for a link factory might be different for your system,
                    // you should probably obtain a factory from the registry.
                    "link-factory",
                    &pw::properties! {
                        "link.output.port" => port_out.id.to_string(),
                        "link.input.port" => port_in.id.to_string(),
                        "link.output.node" => port_out.node.id.to_string(),
                        "link.input.node" => port_in.node.id.to_string(),
                        "object.linger" => "1"
                    },
                ).is_err() {
                    println!("Failed to create link");
                }
            });
    }
}

fn deal_with_node(
    global_object: &pipewire::registry::GlobalObject<libspa::ForeignDict>,
    mut state: RefMut<AppState>,
) {
    if let Some(props) = &global_object.props {
        if let (Some(class), Some(name)) = (props.get("media.class"), props.get("node.name")) {
            if class.starts_with("Audio") {
                if state.get_names {
                    println!(
                        "Got Audio device {}: {}({})",
                        global_object.id,
                        name,
                        props.get("node.nick").unwrap_or("<no nick>")
                    );
                }

                if state.try_add_node(Node {
                    id: global_object.id,
                    name: name.to_string(),
                }) {
                    println!(
                        "Got {}: {}({})",
                        global_object.id,
                        name,
                        props.get("node.nick").unwrap_or("<no nick>")
                    );
                }
            }
        }
    } else {
        println!("No props! Skiping id: {:?}", global_object.id);
    }
}

fn deal_with_port(
    port: &pipewire::registry::GlobalObject<libspa::ForeignDict>,
    mut state: RefMut<AppState>,
    core: Rc<pw::Core>,
) {
    if let Some(props) = &port.props {
        if let (Some(name), Some(node_id)) = (props.get("port.name"), props.get("node.id")) {
            if let Ok(node_id) = node_id.parse::<u32>() {
                if state.try_add_port(port.id, name.to_string(), node_id) {
                    println!(
                        "Got port {} for {}",
                        name,
                        state.get_node(node_id).unwrap().name
                    );
                    state.create_links(name.to_string(), core)
                }
            } else {
                println!("Clould not parse {}'s node.id({})", name, node_id)
            }
        }
    } else {
        println!("No props! Skiping id: {}", port.id);
    }
}

fn help() {
    println!("Usage: \n");
    println!("pw-autoconnect <filename> \n")
}

fn parse_file(path: std::path::PathBuf, get_names: bool) -> Result<AppState, Box<dyn std::error::Error>> {
    let file = fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    
    lazy_static! {
        static ref RE: Regex = Regex::new("\\[(?P<node_out>.*)\\]\\((?P<port_out>.*)\\)\\s*->\\s*\\[(?P<node_in>.*)\\]\\((?P<port_in>.*)\\)").unwrap();
    }

    let mut node_def: HashMap<String, Rc<NodeDef>>  = HashMap::new();
    let mut port_def: HashMap<String, Rc<PortDef>>  = HashMap::new();
    let mut link_def: Vec<Rc<LinkDef>> = Vec::new();

    for line in reader.lines() {
        let line = line?;

        if RE.is_match(&line) {
            let caps = RE.captures(&line).unwrap();
            println!("Found link: [{}]{} -> [{}]{}", &caps["node_out"], &caps["port_out"],  &caps["node_in"], &caps["port_in"]);

            let node_out = match node_def.get_mut(&caps["node_out"]) {
                Some(node) => node.to_owned(),
                None => {
                    let node = Rc::new(NodeDef {name: caps["node_out"].to_string()});
                    node_def.insert(caps["node_out"].to_string(), node.clone());
                    node
                },
            };

            let node_in = match node_def.get_mut(&caps["node_in"]) {
                Some(node) => node.to_owned(),
                None => {
                    let node = Rc::new(NodeDef {name: caps["node_in"].to_string()});
                    node_def.insert(caps["node_in"].to_string(), node.clone());
                    node
                },
            };

            let port_out = match port_def.get_mut(&caps["port_out"]) {
                Some(port) => port.to_owned(),
                None => {
                    let port = Rc::new(PortDef { node: node_out.clone(), name: caps["port_out"].to_string() } );
                    port_def.insert(caps["port_out"].to_string(), port.clone());
                    port
                },
            };

            let port_in = match port_def.get_mut(&caps["port_in"]) {
                Some(port) => port.to_owned(),
                None => {
                    let port = Rc::new(PortDef { node: node_in.clone(), name: caps["port_in"].to_string() } );
                    port_def.insert(caps["port_in"].to_string(), port.clone());
                    port
                },
            };

            let link = Rc::new(LinkDef { port_out: port_out.clone(), port_in: port_in.clone() });

            link_def.push(link)
        } else if !line.starts_with('#') {
            println!("invalid line: {}", line);
        }
    }

    let node_def = node_def.values().cloned().collect::<Vec<Rc<NodeDef>>>();
    let port_def = port_def.values().cloned().collect::<Vec<Rc<PortDef>>>();

    Ok(AppState::new(node_def, link_def, port_def, get_names))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hello, world!");

    let mut args = env::args();
    // Skip the current directory
    args.next();

    let mut find_names = false;

    let mut file_name = None;

    for a in args {
        if a.eq("-f") {
            find_names  = true;
            continue;
        }

        if file_name.is_some() {
            println!("File name already exists");
            return Ok(());
        } else {
            file_name = Some(a);
        }
    }

    if file_name.is_none() {
        help();
        return Ok(());
    }

    let file_name = file_name.unwrap();

    let path = std::path::Path::new(&file_name);

    if !path.exists() || !path.is_file() {
        println!("File not found does not exists");
        return Ok(());
    }

    // Create DeSized State

    let state = RefCell::new(parse_file(path.to_path_buf(), find_names)?);

    println!("\n\nGot state! Starting up\n\n");

    let mainloop = MainLoop::new()?;
    let context = Context::new(&mainloop)?;
    let core = Rc::new(context.connect(None)?);
    let registry = core.get_registry()?;

    let _listener = registry
        .add_listener_local()
        .global(move |global| match global.type_ {
            ObjectType::Port => deal_with_port(global, state.borrow_mut(), core.clone()),
            ObjectType::Node => deal_with_node(global, state.borrow_mut()),
            _ => (),
        })
        .register();

    mainloop.run();

    Ok(())
}
