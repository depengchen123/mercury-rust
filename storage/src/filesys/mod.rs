use ::async::*;
use ::error::*;

use futures::*;
use futures::sync::oneshot;
use std::sync::Arc;
use std::path::Path;
use std::fs::create_dir_all;
use tokio_io::io::*;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::prelude::{Read, Write};
use tokio_fs::*;
use tokio_fs::file::*;
use tokio_threadpool;

pub mod sync;

pub struct AsyncFileHandler{
    path : String,
    pool : tokio_threadpool::ThreadPool,
}

impl AsyncFileHandler{
    pub fn init(main_directory : String) 
    -> Result<Self, StorageError>{
        match create_dir_all(Path::new(&main_directory)){
            Ok(_)=>Ok(
                AsyncFileHandler{
                    path: main_directory, 
                    pool : tokio_threadpool::ThreadPool::new(),
                }
            ),
            Err(e)=>Err(StorageError::Other(Box::new(e)))
        }
    }

    pub fn create_subdir(&self, path : String) 
    -> Result<(), StorageError>{
        match create_dir_all(Path::new(&self.get_path(path))){
            Ok(_)=>Ok(()),
            Err(e)=>Err(StorageError::Other(Box::new(e)))
        }
    }

    pub fn new_file(&self, file_path : String) 
    -> CreateFuture<String>{
        File::create(self.get_path(file_path))
    }
    
    pub fn new_file_with_name(&self, directory_path : String, file_name : String) 
    -> CreateFuture<String>{
        let mut subpath = self.get_path(directory_path);
        subpath.push_str(&file_name);
        File::create(subpath)
    }

    pub fn write_to_file(&self, file_path : String, content : String) 
    -> Box< Future< Item = (), Error = Arc<StorageError> > >{
        let (tx, rx) = oneshot::channel::<Arc<Result<(), StorageError>>>();
        self.pool.spawn(    
            // let mut file_fut = 
            File::create(self.get_path(file_path))
                .or_else(|e| {
                    tx.send(Arc::new(Err(StorageError::StringError(String::from("File couldn't be created")))) );
                    Ok(())
                })
                // .and_then(move |file|{
                //     write_all(file, content.as_bytes())
                //         // .map(|written| {
                //         //     tx.send(Arc::new(Ok(())))
                //         //     // future::ok(())
                //         // } )
                //         // .map_err(|e| {
                //         //     tx.send(Arc::new(Err(StorageError::StringError(String::from("File couldn't be created")))))
                //         //     // future::err(())
                //         // } )
                // })       
                // .map(|_|())
        );
        Box::new(
            rx.map(|_|()).map_err(|e|Arc::new(StorageError::Other(Box::new(e))  ) )
        )
    }

    pub fn read_from_file(&self, file_path : String) 
    -> Box< Future< Item = String, Error = StorageError> > {
        let (tx, rx) = oneshot::channel::<Result<String,StorageError>>();
        if !Path::new(&self.get_path(file_path.clone())).exists(){
            return Box::new(future::err(StorageError::InvalidKey));
        }
        let mut buffer = Vec::new();
        self.pool.spawn({        
            File::open(self.get_path(file_path))
                .or_else(|e|{
                    tx.send(Err(StorageError::StringError(String::from("File couldn't be created"))) );
                    future::err(())
                })
                .and_then(|mut file|{
                    read_to_end(file , buffer)
                        .or_else(|e|{
                            tx.send(Err(StorageError::StringError(String::from("File couldn't be created"))));
                            future::err(())
                        } )
                        .and_then(move |_|{
                            match String::from_utf8(buffer){
                                Ok(content)=>{
                                    tx.send(Ok(content));
                                    future::ok(())
                                }
                                Err(e)=>{
                                    tx.send(Err(StorageError::StringError(String::from("File couldn't be created"))));
                                    future::err(())
                                }
                            }
                        })
                })
        });
        Box::new(
            rx.map_err(|e|Err(StorageError::InvalidKey) )
        )
    }

    pub fn get_path(&self, file_path: String)
    -> String {
        let mut path = self.path.clone();
        path.push_str(&file_path);
        path
    }
}

impl KeyValueStore<String, String> for AsyncFileHandler{
    fn set(&mut self, key: String, value: String)
    -> Box< Future<Item=(), Error=StorageError> >{
        self.write_to_file(key, value)   
    }

    fn get(&self, key: String)
    -> Box< Future<Item=String, Error=StorageError> >{
        self.read_from_file(key)
        // Box::new(self.read_from_file(key).map_err(|e|StorageError::InvalidKey))
    }
}

#[test]
fn future_file_key_value() {
    use tokio_core;
    use tokio_core::reactor;


    let mut reactor = reactor::Core::new().unwrap();
    println!("\n\n\n");
    let mut storage : AsyncFileHandler = AsyncFileHandler::init(String::from("./ipfs/banan/")).unwrap();
    let file_path = String::from("alma.json");
    let json = String::from("<Json:json>");
    let set = storage.set(file_path, json.clone());
    reactor.run(set);
    // reactor.run(storage.set(file_path, String::from("<<profile:almagyar>>")));
    let read = storage.get(file_path);
    let res = reactor.run(read).unwrap();
    assert_eq!(res, json);
}

//test from the tokio_threadpool crate
// #[test]
// fn multi_threadpool() {
//     use futures::sync::oneshot;

//     let pool1 = ThreadPool::new();
//     let pool2 = ThreadPool::new();

//     let (tx, rx) = oneshot::channel();
//     let (done_tx, done_rx) = mpsc::channel();

//     pool2.spawn({
//         rx.and_then(move |_| {
//             done_tx.send(()).unwrap();
//             Ok(())
//         })
//         .map_err(|e| panic!("err={:?}", e))
//     });

//     pool1.spawn(lazy(move || {
//         tx.send(()).unwrap();
//         Ok(())
//     }));

//     done_rx.recv().unwrap();
// }

