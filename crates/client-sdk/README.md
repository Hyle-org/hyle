# Hyle client sdk 

This crates holds some tools for the app interacting with a smart contract.

There is a tool called "Transaction Builder" you can use that helps you build transactions, prove them 
and keep a local state of your app. More documentation to come later. You can look at [HyleOof server](https://github.com/Hyle-org/hyleoof/tree/main)
that uses it.

The `risc0` & `sp1` features enables necessary implementations for the Transaction Builder. Activate 
only the one relevant for your use-case.

The `rest` feature exports a `NodeApiHttpClient` and a `IndexerApiHttpClient` that allows you to call
the node of the indexer on their http endpoints.

The `tcp` feature exports a `NodeTcpClient` that allows you to send transactions to the node using tcp. 
Used for loadtesting purposes.

