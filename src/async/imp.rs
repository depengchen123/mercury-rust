use std::collections::HashMap;
use std::hash::Hash;

use futures::prelude::*;
use futures::future;
use futures_state_stream::StateStream;
use multibase;
use tokio_core::reactor;
use tokio_postgres;

use async::*;



//// Un/Pack storable binary user data and metadata from/to a binary package frame, e.g. ipfs
//pub trait PackageHandler<SerializedType: meta::Data, PackageType>
//{
//    fn link_prefix(&self) -> &str;
//    fn pack(&self, data: SerializedType, attributes: Box< Iterator<&Attribute> >)
//            -> Result<PackageType, HashSpaceError>;
//    fn unpack(&self, package: PackageType) -> Result<SerializedType, HashSpaceError>;
//}
//
//
//pub struct HashWeb
//{
//    spaces: HashMap< StorageId, Box< HashSpace< Rc<Vec<u8>> > > >,
//}
//
//
//impl HashWeb
//{
//    pub fn new(spaces: HashMap< StorageId, Box< HashSpace< Rc<Vec<u8>> > > >) -> Self
//        { HashWeb{spaces: spaces} }
//
//
//    pub fn resolve(&self, link: &Link) -> Box< Future<Item = Rc<Data>, Error = HashSpaceError > >
//    {
//        let storage_res = self.spaces.get( &link.storage() )
//            .ok_or( HashSpaceError::UnsupportedStorage( link.storage() ) );
//        let storage = match storage_res {
//            Ok(ref storage) => &storage,
//            Err(e) => return Box::new( future::err(e) ),
//        };
//        let data = storage.resolve( &link.hash() );
//        data
//    }
//}



pub struct InMemoryStore<KeyType, ValueType>
{
    map: HashMap<KeyType, ValueType>,
}

impl<KeyType, ValueType> InMemoryStore<KeyType, ValueType>
    where KeyType: Eq + Hash
{
    pub fn new() -> Self { InMemoryStore{ map: HashMap::new() } }
}

impl<KeyType, ValueType>
KeyValueStore<KeyType, ValueType>
for InMemoryStore<KeyType, ValueType>
    where KeyType: Eq + Hash,
          ValueType: Clone + 'static
{
    fn store(&mut self, key: KeyType, object: ValueType)
        -> Box< Future<Item=(), Error=StorageError> >
    {
        self.map.insert(key, object );
        Box::new( future::ok(() ) )
    }

    fn lookup(&self, key: KeyType)
        -> Box< Future<Item=ValueType, Error=StorageError> >
    {
        let result = match self.map.get(&key) {
            Some(val) => future::ok( val.to_owned() ),
            None      => future::err(StorageError::InvalidKey),
        };
        Box::new(result)
    }
}



pub struct PostgresStore
{
    reactor_handle: reactor::Handle,
    postgres_url:   String,
    table:          String,
    key_col:        String,
    value_col:      String,
}

impl PostgresStore
{
    pub fn new(reactor_handle: &reactor::Handle, postgres_url: &str,
               table: &str, key_col: &str, value_col: &str) -> Self
    {
        Self{ reactor_handle:   reactor_handle.clone(),
              postgres_url:     postgres_url.to_string(),
              table:            table.to_string(),
              key_col:          key_col.to_string(),
              value_col:        value_col.to_string(), }
    }

    fn prepare(&self, sql_statement: String)
        -> Box< Future<Item=(tokio_postgres::stmt::Statement,tokio_postgres::Connection), Error=(tokio_postgres::Error,tokio_postgres::Connection)> >
    {
        let result = tokio_postgres::Connection::connect( self.postgres_url.as_str(),
                tokio_postgres::TlsMode::None, &self.reactor_handle)
            .then( move |conn| {
                conn.expect("Connection to database failed")
                    .prepare(&sql_statement)
            } );
        Box::new(result)
    }
}

impl KeyValueStore<Vec<u8>, Vec<u8>> for PostgresStore
{
    fn store(&mut self, key: Vec<u8>, value: Vec<u8>)
        -> Box< Future<Item=(), Error=StorageError> >
    {
        let key_str = multibase::encode(multibase::Base64, &key);
        let sql = format!("INSERT INTO {0} ({1}, {2}) VALUES ($1, $2)",
            self.table, self.key_col, self.value_col);
        let result = self.prepare(sql)
            .and_then( move |(stmt, conn)| {
                conn.execute(&stmt, &[&key_str, &value])
            } )
            .map_err( |(e,_conn)| StorageError::Other( Box::new(e) ) )
            .map( |_exec_res| () );
        Box::new(result)
    }

    fn lookup(&self, key: Vec<u8>)
        -> Box< Future<Item=Vec<u8>, Error=StorageError> >
    {
        let key_str = multibase::encode(multibase::Base64, &key);
        let sql = format!("SELECT {1}, {2} FROM {0} WHERE {1}=$1",
            self.table, self.key_col, self.value_col);
        let result = self.prepare( sql.to_string() )
            .and_then( move |(stmt, conn)|
                conn.query(&stmt, &[&key_str])
                    .map( |row| {
                        let value: Vec<u8> = row.get(1);
                        value
                    } )
                    .collect()
                    // TODO reducing resulsts by concatenating provides bad results
                    //      if multiple rows are found in the result set
                    .map( |(vec,_state)| vec.concat() )
            )
            .map_err( |(e,_conn)| StorageError::Other( Box::new(e) ) );

        Box::new(result)
    }
}



#[cfg(test)]
mod tests
{
    use multihash;
    use tokio_core::reactor;

    use common::imp::*;
    use super::*;


    #[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
    struct Person
    {
        name:  String,
        phone: String,
        age:   u16,
    }


    #[test]
    fn test_inmemory_storage()
    {
        // NOTE this works without a tokio::reactor::Core only because
        //      the storage always returns an already completed future::ok/err result
        let object = Person{ name: "Aladar".to_string(), phone: "+36202020202".to_string(), age: 28 };
        let hash = "key".to_string();
        let mut storage: InMemoryStore<String,Person> = InMemoryStore::new();
        let store_res = storage.store( hash.clone(), object.clone() ).wait();
        assert!( store_res.is_ok() );
        let lookup_res = storage.lookup(hash).wait();
        assert!( lookup_res.is_ok() );
        assert_eq!( lookup_res.unwrap(), object );
    }


    #[test]
    fn test_hashspace()
    {
        // NOTE this works without a tokio::reactor::Core only because
        //      all plugins always return an already completed ok/err result
        let store: InMemoryStore<Vec<u8>, Vec<u8>> = InMemoryStore::new();
        let mut hashspace: ModularHashSpace<Person, Vec<u8>, Vec<u8>, String> = ModularHashSpace::new(
            Rc::new( SerdeJsonSerializer{} ),
            Rc::new( MultiHasher::new(multihash::Hash::Keccak512) ),
            Box::new(store),
            Box::new( MultiBaseHashCoder::new(multibase::Base64) ) );

        let object = Person{ name: "Aladar".to_string(), phone: "+36202020202".to_string(), age: 28 };
        let store_res = hashspace.store( object.clone() ).wait();
        assert!( store_res.is_ok() );
        let hash = store_res.unwrap();
        let lookup_res = hashspace.resolve(&hash).wait();
        assert!( lookup_res.is_ok() );
        assert_eq!( lookup_res.unwrap(), object );
//        let validate_res = hashspace.validate(&object, &hash).wait();
//        assert!( validate_res.is_ok() );
//        assert!( validate_res.unwrap() );
    }


    #[test]
    fn test_postgres_storage()
    {
        // TODO consider if these should also use assert!() calls instead of expect/unwrap
        let mut reactor = reactor::Core::new()
            .expect("Failed to initialize the reactor event loop");

        // This URL should be assembled from the gitlab-ci.yml
        let postgres_url = "postgresql://testuser:testpass@postgres/testdb";
        let mut storage = PostgresStore::new( &reactor.handle(),
            postgres_url, "storagetest", "key", "data");

        let key = b"key".to_vec();
        let value = b"value".to_vec();
        let store_future = storage.store( key.clone(), value.clone() );
        let store_res = reactor.run(store_future);
        assert!( store_res.is_ok(), "store failed with {:?}", store_res );

        let lookup_future = storage.lookup(key);
        let lookup_res = reactor.run(lookup_future);
        assert!( lookup_res.is_ok(), "lookup failed with {:?}", lookup_res );
        assert_eq!( lookup_res.unwrap(), value );
    }
}
