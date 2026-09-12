# Owned interface-address snapshot

`getifaddrs` returns a linked list whose storage must be released with
`freeifaddrs`. The typed API keeps every native pointer private:

```rust
let addresses = agenterm_dyn::InterfaceAddresses::acquire()?;
for address in addresses.snapshot()? {
    println!("{:?}: family={:?} flags={:#x}", address.name, address.address_family, address.flags);
}
```

Each snapshot contains only copied names, flags, and optional address-family
numbers. Dropping `addresses` calls `freeifaddrs` exactly once. Windows returns
`InterfaceAddressesError::Unsupported`; its catalog row remains a placeholder.
