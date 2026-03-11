{- |
Module      : Main
Description : Test vector generator for Torsten formal ledger conformance tests
License     : Apache-2.0

This program generates test vectors by calling the Agda-compiled step functions
from the formal Cardano ledger specification. It produces JSON files that can
be consumed by the Rust conformance test harness in torsten-conformance.

= Prerequisites

This requires building the formal-ledger-specifications project with Nix:

  1. Clone: git clone https://github.com/IntersectMBO/formal-ledger-specifications.git
  2. Checkout: git checkout conway-v1.0
  3. Build:   nix build .#cardano-ledger-executable-spec

= Usage

  cabal run conformance-generator -- --output-dir ../vectors/

= Architecture

The Agda formal specification defines STS (State Transition System) rules:

  - UTXO: UTxO validation and state transitions
  - CERT: Certificate processing (delegation, registration)
  - GOV:  Governance actions (Conway era)
  - EPOCH: Epoch boundary transitions

Each rule has a step function of the form:

  step :: Environment -> State -> Signal -> Either Error State

This generator:
  1. Constructs test inputs using the Agda API types
  2. Calls the step function
  3. Serializes the (env, state, signal, result) tuple as JSON
  4. Writes to the output directory

-}
module Main where

-- TODO: When the Nix build environment is available, uncomment these imports
-- and implement the generator functions.
--
-- import qualified Lib          -- from cardano-ledger-executable-spec
-- import Data.Aeson (encode, object, (.=))
-- import qualified Data.ByteString.Lazy as BSL
-- import System.FilePath ((</>))

main :: IO ()
main = do
  putStrLn "Conformance test vector generator"
  putStrLn "================================="
  putStrLn ""
  putStrLn "This generator requires the Agda-compiled Haskell libraries from"
  putStrLn "formal-ledger-specifications. See README.md for build instructions."
  putStrLn ""
  putStrLn "To build with Nix:"
  putStrLn "  nix develop github:IntersectMBO/formal-ledger-specifications/conway-v1.0"
  putStrLn "  cabal run conformance-generator -- --output-dir ../vectors/"
  putStrLn ""
  putStrLn "For now, use the hand-crafted test vectors in vectors/"

-- | TODO: Generate UTXO test vectors
--
-- Pattern from formal-ledger-specifications/conformance-example/test/UtxowSpec.hs:
--
--   genUtxoTestCase :: Gen ConformanceTestVector
--   genUtxoTestCase = do
--     env   <- genUtxoEnv
--     state <- genUtxoState
--     tx    <- genTransaction
--     let result = utxoStep env state tx
--     pure $ ConformanceTestVector
--       { rule = "UTXO"
--       , description = "Generated UTXO test case"
--       , environment = toJSON env
--       , inputState = toJSON state
--       , signal = toJSON tx
--       , expectedOutput = case result of
--           Right newState -> Success (toJSON newState)
--           Left errs      -> Failure (map show errs)
--       }

-- | TODO: Generate CERT test vectors
--
--   genCertTestCase :: Gen ConformanceTestVector
--   genCertTestCase = do
--     env   <- genCertEnv
--     state <- genCertState
--     cert  <- genCertificate
--     let result = certStep env state cert
--     pure $ ...
