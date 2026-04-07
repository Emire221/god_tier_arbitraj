---
agent: agent
description: "Solidity kontratı için Foundry test suite yazar - unit, fuzz, fork tests"
---

# 🧪 Solidity Test Yazımı (Foundry)

## Görev
Seçili Solidity kodu için **kapsamlı Foundry test suite** oluştur.

## Test Kategorileri

### 1. Unit Tests
- Her public/external fonksiyon
- Revert koşulları
- Edge case'ler

### 2. Fuzz Tests
- Random input ile property testing
- Boundary conditions
- Invariant kontrolü

### 3. Fork Tests
- Mainnet/Base fork ile gerçek state
- Integration testing
- E2E arbitraj senaryoları

## Test Şablonu

```solidity
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.27;

import {Test, console2} from "forge-std/Test.sol";
import {ArbitrageExecutor} from "../src/ArbitrageExecutor.sol";

contract ArbitrageExecutorTest is Test {
    ArbitrageExecutor public executor;

    address constant EXECUTOR_EOA = address(0x1);
    address constant ADMIN = address(0x2);
    address constant ATTACKER = address(0x3);

    function setUp() public {
        executor = new ArbitrageExecutor(EXECUTOR_EOA, ADMIN);
        vm.deal(address(executor), 10 ether);
    }

    // ===== ACCESS CONTROL TESTS =====

    function test_OnlyExecutorCanCall() public {
        vm.prank(EXECUTOR_EOA);
        // executor.execute(...);  // Should succeed
    }

    function test_RevertWhen_UnauthorizedCaller() public {
        vm.prank(ATTACKER);
        vm.expectRevert(ArbitrageExecutor.Unauthorized.selector);
        // executor.execute(...);
    }

    // ===== REENTRANCY TESTS =====

    function test_RevertWhen_Reentrancy() public {
        // Reentrancy attack simulation
    }

    // ===== FUZZ TESTS =====

    function testFuzz_ProfitCalculation(uint256 inputAmount) public {
        // Bound input to reasonable range
        inputAmount = bound(inputAmount, 0.01 ether, 100 ether);

        // Property: output should be >= input for profitable arb
        // ...
    }

    function testFuzz_CallbackValidation(address caller) public {
        vm.assume(caller != address(pool));  // Exclude valid caller

        vm.prank(caller);
        vm.expectRevert(ArbitrageExecutor.InvalidCaller.selector);
        // executor.uniswapV3SwapCallback(...);
    }

    // ===== FORK TESTS =====

    function test_Fork_RealArbitrage() public {
        // Fork Base mainnet
        vm.createSelectFork(vm.envString("BASE_RPC_URL"));

        // Real pool addresses
        // Real arbitrage execution
    }

    // ===== GAS TESTS =====

    function test_GasUsage_Execute() public {
        vm.prank(EXECUTOR_EOA);
        uint256 gasBefore = gasleft();
        // executor.execute(...);
        uint256 gasUsed = gasBefore - gasleft();

        console2.log("Gas used:", gasUsed);
        assertLt(gasUsed, 500_000, "Gas usage too high");
    }

    // ===== INVARIANT TESTS =====

    function invariant_ContractBalanceNeverZero() public {
        // After setup, contract should always have some balance
        // (unless explicitly withdrawn)
    }
}
```

## Test Senaryoları

### Arbitraj Spesifik
```solidity
function test_ArbitrageProfit() public {
    // Setup: Pool A price < Pool B price
    // Action: Buy from A, sell to B
    // Assert: Contract balance increased
}

function test_RevertWhen_InsufficientProfit() public {
    // Setup: minProfit = 0.1 ETH
    // Action: Execute arb with 0.05 ETH profit
    // Assert: Revert with InsufficientProfit
}

function test_RevertWhen_StalePoolData() public {
    // Setup: Pool state older than threshold
    // Assert: Revert or skip
}
```

### Callback Güvenliği
```solidity
function test_CallbackOnlyFromExpectedPool() public {
    // Setup: tstore expected pool address
    // Action: Callback from different address
    // Assert: Revert with InvalidCaller
}
```

## Çalıştırma Komutları
```bash
# Tüm testler
forge test -vvv

# Belirli test
forge test --match-test test_ArbitrageProfit -vvvv

# Gas raporu
forge test --gas-report

# Fork test
forge test --fork-url $BASE_RPC_URL

# Fuzz runs artırma
forge test --fuzz-runs 10000

# Coverage
forge coverage
```

## Proje Bağlamı
- Framework: Foundry
- Solidity: ^0.8.27
- Network: Base L2
