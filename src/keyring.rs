use crate::ffi::{self, KeyCtlOperation};
use crate::utils::{CStr, CString, Vec};
use crate::{Key, KeyError, KeyRingIdentifier, KeySerialId, KeyType, LinkNode, Links, Metadata};
use core::convert::TryInto;

/// Interface to perform keyring operations. Used to locate, create,
/// search, add, and link/unlink keys to & from keyrings.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct KeyRing {
    id: KeySerialId,
}

impl KeyRing {
    /// Initialize a new [KeyRing] object from the provided ID
    pub(crate) fn from_id(id: KeySerialId) -> Self {
        Self { id }
    }

    /// Obtain a KeyRing from its special identifier.
    ///
    /// If the create argument is true, then this method will attempt
    /// to create the keyring. Otherwise it will only succeed if the
    /// keyring already exists and is valid.
    ///
    /// Internally this uses KEYCTL_GET_KEYRING_ID to resolve a keyrings
    /// real ID from the special identifier.
    pub fn from_special_id(id: KeyRingIdentifier, create: bool) -> Result<Self, KeyError> {
        let id: KeySerialId = ffi::keyctl!(
            KeyCtlOperation::GetKeyRingId,
            id as libc::c_ulong,
            u32::from(create).into()
        )?
        .try_into()
        .or(Err(KeyError::InvalidIdentifier))?;
        Ok(Self { id })
    }

    /// Get the persistent keyring  (persistent-keyring(7)) of the current user
    /// and link it to a specified keyring.
    ///
    /// If the call is successful, a link to the persistent keyring is added to the
    /// keyring specified in the `link_with` argument.
    ///
    /// The caller must have write permission on the keyring.
    ///
    /// The persistent keyring will be created by the kernel if it does not yet exist.
    ///
    /// Each time the [KeyRing::get_persistent] operation is performed, the persistent
    /// keyring will have its expiration timeout reset to the value in:
    ///
    ///    `/proc/sys/kernel/keys/persistent_keyring_expiry`
    ///
    /// Should the timeout be reached, the persistent keyring will be removed and
    /// everything it pins can then be garbage collected.
    ///
    /// Persistent keyrings were added to Linux in kernel version 3.13.
    pub fn get_persistent(link_with: KeyRingIdentifier) -> Result<Self, KeyError> {
        let id: KeySerialId = ffi::keyctl!(
            KeyCtlOperation::GetPersistent,
            u32::MAX as _,
            link_with as libc::c_ulong
        )?
        .try_into()
        .or(Err(KeyError::InvalidIdentifier))?;
        Ok(Self { id })
    }

    /// Obtain information describing the attributes of this keyring.
    ///
    /// The keyring must grant the caller view permission.
    pub fn metadata(&self) -> Result<Metadata, KeyError> {
        Metadata::from_id(self.id)
    }

    /// Creates or updates a key of the given description and User type,
    /// instantiates it with the payload of length plen, attaches it to the
    /// keyring.
    ///
    /// If the destination keyring already contains a key that matches
    /// the specified type and description, then, if the key type supports
    /// it, that key will be updated rather than a new key being created;
    /// if not, a new key (with a different ID) will be created and it will
    /// displace the link to the extant key from the keyring.
    pub fn add_key<D: AsRef<str> + ?Sized, S: AsRef<[u8]> + ?Sized>(
        &self,
        description: &D,
        secret: &S,
    ) -> Result<Key, KeyError> {
        let id = ffi::add_key(
            KeyType::User,
            self.id.as_raw_id() as libc::c_ulong,
            description.as_ref(),
            Some(secret.as_ref()),
        )?;
        Ok(Key::from_id(id))
    }

    /// Attempts to find a key of the given type with a description that
    /// matches the specified description. If such a key could not be found,
    /// then the key is optionally created.
    ///
    /// If the key is found or created, it is attached it to the keyring
    /// and returns the key's serial number.
    ///
    /// If the key is not found and callout info is empty then the call
    /// fails with the error ENOKEY.
    ///
    /// If the key is not found and callout info is not empty, then the
    /// kernel attempts to invoke a user-space program to instantiate the
    /// key.
    pub fn request_key<D: AsRef<str> + ?Sized, C: AsRef<str> + ?Sized>(
        &self,
        description: &D,
        callout: Option<&C>,
    ) -> Result<Key, KeyError> {
        let id = ffi::request_key(
            KeyType::User,
            self.id.as_raw_id() as libc::c_ulong,
            description.as_ref(),
            callout.map(|c| c.as_ref()),
        )?;
        Ok(Key::from_id(id))
    }

    /// Search for a key in the keyring tree, starting with this keyring as the head,
    /// returning its ID.
    ///
    /// The search is performed breadth-first and recursively.
    ///
    /// The source keyring must grant search permission to the caller. When
    /// performing the recursive search, only keyrings that grant the caller search
    /// permission will be searched. Only keys with for which the caller has
    /// search permission can be found.
    ///
    /// If the key is found, its ID is returned as the function result.
    pub fn search<D: AsRef<str> + ?Sized>(&self, description: &D) -> Result<Key, KeyError> {
        // The provided description must be properly null terminated for the kernel
        let description =
            CString::new(description.as_ref()).or(Err(KeyError::InvalidDescription))?;

        // Perform the raw syscall and validate that the result is a valid ID
        let id: KeySerialId = ffi::keyctl!(
            KeyCtlOperation::Search,
            self.id.as_raw_id() as libc::c_ulong,
            Into::<&'static CStr>::into(KeyType::User).as_ptr() as _,
            description.as_ptr() as _,
            0
        )?
        .try_into()
        .or(Err(KeyError::InvalidIdentifier))?;

        // Construct a key object from the ID
        Ok(Key::from_id(id))
    }

    /// Obtain a list of the keys/keyrings linked to this keyring.
    ///
    /// This method allocates, but you can provide a maximum number of entries
    /// to read. Each returned entry is 4 bytes.
    ///
    /// The keyring must either grant the caller read permission, or grant
    /// the caller search permission.
    pub fn get_links(&self, max: usize) -> Result<Links, KeyError> {
        // Allocate the requested capacity
        let mut buffer = Vec::<KeySerialId>::with_capacity(max);

        // Perform the read
        let len = ffi::keyctl!(
            KeyCtlOperation::Read,
            self.id.as_raw_id() as libc::c_ulong,
            buffer.as_mut_ptr() as _,
            buffer.capacity() as _
        )? as usize;

        // Set the size of the results
        unsafe {
            buffer.set_len(len / core::mem::size_of::<KeySerialId>());
        }

        // Remap the results to complete keys
        Ok(buffer
            .iter()
            .filter_map(|&id| LinkNode::from_id(id).ok())
            .collect())
    }

    /// Create a link from this keyring to a key.
    ///
    /// If a key with the same type and description is already linked in the keyring,
    /// then that key is displaced from the keyring.
    ///
    /// Before  creating  the  link,  the  kernel  checks the nesting of the keyrings
    /// and returns appropriate errors if the link would produce a cycle or if the
    /// nesting of keyrings would be too deep (The limit on the nesting of keyrings is
    /// determined by the kernel constant KEYRING_SEARCH_MAX_DEPTH, defined with the
    /// value 6, and is necessary to prevent overflows on the kernel stack when
    /// recursively searching keyrings).
    ///
    /// The caller must have link permission on the key being added and write
    /// permission on the keyring.
    pub fn link_key(&self, key: Key) -> Result<(), KeyError> {
        _ = ffi::keyctl!(
            KeyCtlOperation::Link,
            key.get_id().as_raw_id() as _,
            self.id.as_raw_id() as libc::c_ulong
        )?;
        Ok(())
    }

    /// Unlink a key from this keyring.
    ///
    /// If the key is not currently linked into the keyring, an error results. If the
    /// last link to a key is removed, then that key will be scheduled for destruction.
    ///
    /// The caller must have write permission on the keyring from which the key is being
    /// removed.
    pub fn unlink_key(&self, key: Key) -> Result<(), KeyError> {
        _ = ffi::keyctl!(
            KeyCtlOperation::Unlink,
            key.get_id().as_raw_id() as _,
            self.id.as_raw_id() as libc::c_ulong
        )?;
        Ok(())
    }

    /// Link another keyring to this keyring.
    ///
    /// Behaves similarly to link_key, but links a KeyRing instead. The caller
    /// must have link permission on the keyring being added as a link, and
    /// write permission on this keyring.
    pub fn link_keyring(&self, keyring: KeyRing) -> Result<(), KeyError> {
        _ = ffi::keyctl!(
            KeyCtlOperation::Link,
            keyring.id.as_raw_id() as libc::c_ulong,
            self.id.as_raw_id() as libc::c_ulong
        )?;
        Ok(())
    }

    /// Unlink another keyring from this keyring.
    ///
    /// Behaves similarly to unlink_key, but unlinks a KeyRing instead. The
    /// caller must have write permission on the keyring to remove links
    /// from it.
    pub fn unlink_keyring(&self, keyring: KeyRing) -> Result<(), KeyError> {
        _ = ffi::keyctl!(
            KeyCtlOperation::Unlink,
            keyring.id.as_raw_id() as libc::c_ulong,
            self.id.as_raw_id() as libc::c_ulong
        )?;
        Ok(())
    }

    /// Link a default keyring from this keyring.
    ///
    /// This method does the same thing as link_keyring, but links one of the
    /// special keyrings defined by the system. This is useful when you
    /// don't want to have to open a keyring before linking it.
    ///
    /// The caller must have link permissions on the added keyring, and write
    /// permission on this keyring. Requesting to link to a non-existent default
    /// keyring will result in that keyring being created automatically.
    pub fn link_keyring_id(&self, keyringid: KeyRingIdentifier) -> Result<(), KeyError> {
        _ = ffi::keyctl!(
            KeyCtlOperation::Link,
            keyringid as libc::c_ulong,
            self.id.as_raw_id() as libc::c_ulong
        )?;
        Ok(())
    }

    /// Unlink a default keyring from this keyring.
    ///
    /// This method does the same thing as unlink_keyring, but unlinks one of
    /// the special keyrings defined by the system. This is useful when you
    /// don't want to have to open a keyring before unlinking it.
    ///
    /// The caller must have write permission on this keyring. In addition, this
    /// method will return KeyError::KeyDoesNotExist if the target keyring has
    /// not yet been created.
    pub fn unlink_keyring_id(&self, keyringid: KeyRingIdentifier) -> Result<(), KeyError> {
        _ = ffi::keyctl!(
            KeyCtlOperation::Unlink,
            keyringid as libc::c_ulong,
            self.id.as_raw_id() as libc::c_ulong
        )?;
        Ok(())
    }

    /// Clear the contents of (i.e., unlink all keys from) this keyring.
    ///
    /// The caller must have write permission on the keyring.
    pub fn clear(&self) -> Result<(), KeyError> {
        _ = ffi::keyctl!(KeyCtlOperation::Clear, self.id.as_raw_id() as libc::c_ulong)?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{KeyPermissionsBuilder, Permission};

    #[test]
    fn test_from_special_id() {
        // Test that a keyring that normally doesn't exist by default is
        // created when called.
        let ring = KeyRing::from_special_id(KeyRingIdentifier::Thread, true).unwrap();
        assert!(ring.id.as_raw_id() > 0);

        // Test that a keyring that should already exist is returned
        let ring = KeyRing::from_special_id(KeyRingIdentifier::User, false).unwrap();
        assert!(ring.id.as_raw_id() > 0);
    }

    #[test]
    fn test_get_persistent() {
        // Test that a keyring that should already exist is returned
        let user_ring = KeyRing::from_special_id(KeyRingIdentifier::User, false).unwrap();
        assert!(user_ring.id.as_raw_id() > 0);

        let user_perm_ring = KeyRing::get_persistent(KeyRingIdentifier::User).unwrap();
        assert_ne!(user_ring, user_perm_ring);
    }

    #[test]
    fn test_metadata() {
        // Test that a keyring that normally doesn't exist by default is
        // created when called.
        let ring = KeyRing::from_special_id(KeyRingIdentifier::Thread, true).unwrap();
        assert!(ring.id.as_raw_id() > 0);

        // Obtain and verify the info
        let info = ring.metadata().unwrap();
        assert_eq!(info.get_type(), KeyType::KeyRing);
        assert_eq!(info.get_uid(), unsafe { libc::geteuid() });
        assert_eq!(info.get_gid(), unsafe { libc::getegid() });
        assert_eq!(info.get_description(), "_tid");
    }

    #[test]
    fn test_search_existing_key() {
        // Test that a keyring that normally doesn't exist by default is
        // created when called.
        let ring = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();
        let key = ring.add_key("test_search", b"data").unwrap();

        // Ensure we have search permission on the key
        let perms = KeyPermissionsBuilder::builder()
            .posessor(Permission::ALL)
            .user(Permission::ALL)
            .build();

        // Enforce perms
        key.set_perms(perms).unwrap();

        // Search should succeed
        let result = ring.search("test_search").unwrap();

        // Assert that the ID is the same
        assert_eq!(key.get_id(), result.get_id());

        // Request should also succeed
        let result = ring.request_key("test_search", None::<&str>).unwrap();

        // Assert that the ID is the same
        assert_eq!(key.get_id(), result.get_id());

        // Invalidate the key
        key.invalidate().unwrap();
    }

    #[test]
    fn test_request_non_existing_key() {
        // Test that a keyring that normally doesn't exist by default is
        // created when called.
        let ring = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();

        let result = ring.request_key("test_request_no_exist", None::<&str>);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), KeyError::KeyDoesNotExist);
    }

    #[test]
    #[ignore]
    fn test_request_non_existing_key_callout() {
        let callout = "Test Data from Callout";

        // Test that a keyring that normally doesn't exist by default is
        // created when called.
        let ring = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();

        // The test expects that the key is instantiated by a program invoked by
        // /sbin/request-key and that the key data is taken from the callout info
        // passed here.
        //
        // The following examples/keyctl command in /etc/request-key.conf is known to work:
        // create	user	test_callout	*		/path/to/examples/keyctl instantiate --keyid %k --payload %c --ring %S
        let key = ring.request_key("test_callout", Some(callout)).unwrap();

        // Verify the payload
        let payload = key.read_to_vec().unwrap();
        assert_eq!(callout.as_bytes(), &payload);

        // Invalidate the key
        key.invalidate().unwrap();
    }

    #[test]
    fn test_search_non_existing_key() {
        // Test that a keyring that normally doesn't exist by default is
        // created when called.
        let ring = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();

        // Search should succeed
        let result = ring.search("test_search_no_exist");

        // Assert that the ID is the same
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), KeyError::KeyDoesNotExist);
    }

    #[test]
    fn test_link_unlink_keyrings() {
        // Get a couple of unlinked keyrings to test
        let sess = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();
        assert!(sess.id.as_raw_id() > 0);
        let thread = KeyRing::from_special_id(KeyRingIdentifier::Thread, true).unwrap();
        assert!(thread.id.as_raw_id() > 0);

        // Assert that the keyrings are not linked
        let items = sess.get_links(200).unwrap();
        assert!(!items.contains(&thread));

        // Link the keyrings
        let _ = sess.link_keyring(thread).unwrap();

        // Assert that the keyrings are now linked
        let items = sess.get_links(200).unwrap();
        assert!(items.contains(&thread));

        // Unlink the keyrings
        let _ = sess.unlink_keyring(thread).unwrap();

        // Assert that the keyrings are unlinked again
        let items = sess.get_links(200).unwrap();
        assert!(!items.contains(&thread));
    }
    #[test]
    fn test_link_unlink_keyrings_with_id() {
        // Get a couple of unlinked keyrings to test
        let sess = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();
        assert!(sess.id.as_raw_id() > 0);
        let thread = KeyRing::from_special_id(KeyRingIdentifier::Thread, true).unwrap();
        assert!(thread.id.as_raw_id() > 0);

        // Assert that the keyrings are not linked
        let items = sess.get_links(200).unwrap();
        assert!(!items.contains(&thread));

        // Link the keyrings
        let _ = sess.link_keyring_id(KeyRingIdentifier::Thread).unwrap();

        // Assert that the keyrings are now linked
        let items = sess.get_links(200).unwrap();
        assert!(items.contains(&thread));

        // Unlink the keyrings
        let _ = sess.unlink_keyring_id(KeyRingIdentifier::Thread).unwrap();

        // Assert that the keyrings are unlinked again
        let items = sess.get_links(200).unwrap();
        assert!(!items.contains(&thread));
    }

    #[test]
    fn test_linking_nonexistent_keyrings() {
        // Get existent keyring
        let sess = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();
        assert!(sess.id.as_raw_id() > 0);

        // Test that the target keyring doesn't exist
        let thread = KeyRing::from_special_id(KeyRingIdentifier::Thread, false);
        assert!(matches!(thread, Err(KeyError::KeyDoesNotExist)));

        // Unlinking a non-existent keyring
        let result = sess.unlink_keyring_id(KeyRingIdentifier::Thread);
        assert!(matches!(result, Err(KeyError::KeyDoesNotExist)));

        // Linking a non-existent keyring
        sess.link_keyring_id(KeyRingIdentifier::Thread).unwrap();

        // After attempting to link the special keyring, it will have been created
        let sess = KeyRing::from_special_id(KeyRingIdentifier::Thread, false).unwrap();
        assert!(sess.id.as_raw_id() > 0);
    }

    #[test]
    fn test_get_linked_items() {
        // Test that a keyring that should already exist is returned
        let ring = KeyRing::from_special_id(KeyRingIdentifier::Session, false).unwrap();
        assert!(ring.id.as_raw_id() > 0);

        // Add the key
        let key = ring.add_key("test_read_key", b"test").unwrap();

        // Obtain a list of the linked keys
        let items = ring.get_links(200).unwrap();

        // Assert that the key is in the ring
        assert!(items.len() > 0);
        assert!(items.contains(&key));

        // Use the alternate reference to the key
        let key_ref = items.get(&key).unwrap().as_key().unwrap();

        // Invalidate the key
        key_ref.invalidate().unwrap();

        // Assert that the key is no longer on the ring
        let items = ring.get_links(200).unwrap();
        assert!(!items.contains(&key));
    }
}
