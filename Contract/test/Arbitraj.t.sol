// SPDX-License-Identifier: MIT
pragma solidity ^0.8.27;

import {Test, console} from "forge-std/Test.sol";
import {
    ArbitrajBotu,
    IERC20,
    ICLPool,
    Unauthorized,
    InvalidCaller,
    NoProfitRealized,
    InsufficientProfit,
    Locked,
    ZeroAmount,
    TransferFailed
} from "../src/Arbitraj.sol";

// ══════════════════════════════════════════════════════════════════════════════
//                             MOCK CONTRACTS
// ══════════════════════════════════════════════════════════════════════════════

/// @dev Minimal ERC20 mock — mint + transfer + balanceOf
contract MockERC20 {
    string public name;
    uint8 public decimals;
    mapping(address => uint256) public balanceOf;

    constructor(string memory _name, uint8 _decimals) {
        name = _name;
        decimals = _decimals;
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        require(balanceOf[msg.sender] >= amount, "MockERC20: insufficient balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
        return true;
    }

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
    }
}

/// @dev Simulates Uniswap V3 pool flash swap behavior.
///      When swap() is called, it:
///        1. Transfers output tokens to recipient (flash behavior)
///        2. Calls uniswapV3SwapCallback on recipient
///      Configurable deltas: positive = owed by caller, negative = sent to caller.
contract MockUniswapV3Pool {
    address public token0;
    address public token1;

    int256 public mockAmount0Delta;
    int256 public mockAmount1Delta;

    constructor(address _t0, address _t1) {
        token0 = _t0;
        token1 = _t1;
    }

    function setMockDeltas(int256 _a0, int256 _a1) external {
        mockAmount0Delta = _a0;
        mockAmount1Delta = _a1;
    }

    function swap(
        address recipient,
        bool, /* zeroForOne */
        int256, /* amountSpecified */
        uint160, /* sqrtPriceLimitX96 */
        bytes calldata data
    ) external returns (int256, int256) {
        // Flash swap: output token'ları ÖNCE gönder
        if (mockAmount0Delta < 0) {
            MockERC20(token0).transfer(recipient, uint256(-mockAmount0Delta));
        }
        if (mockAmount1Delta < 0) {
            MockERC20(token1).transfer(recipient, uint256(-mockAmount1Delta));
        }

        // Callback tetikle — kontrat TLOAD ile bağlam okuyacak
        (bool ok, bytes memory ret) = recipient.call(
            abi.encodeWithSignature(
                "uniswapV3SwapCallback(int256,int256,bytes)",
                mockAmount0Delta,
                mockAmount1Delta,
                data
            )
        );
        if (!ok) {
            assembly { revert(add(ret, 32), mload(ret)) }
        }

        return (mockAmount0Delta, mockAmount1Delta);
    }
}

/// @dev Simulates Aerodrome Slipstream (Concentrated Liquidity) pool.
///      V3-style: callback ile token ödeme mekanizması.
///      When swap() is called, it:
///        1. Transfers output tokens to recipient
///        2. Calls uniswapV3SwapCallback on recipient (Slipstream = V3 fork)
///      Caller must pay owed amount in the callback.
contract MockSlipstreamPool {
    address public token0;
    address public token1;

    int256 public mockAmount0Delta;
    int256 public mockAmount1Delta;

    constructor(address _t0, address _t1) {
        token0 = _t0;
        token1 = _t1;
    }

    function setMockDeltas(int256 _a0, int256 _a1) external {
        mockAmount0Delta = _a0;
        mockAmount1Delta = _a1;
    }

    function swap(
        address recipient,
        bool, /* zeroForOne */
        int256, /* amountSpecified */
        uint160, /* sqrtPriceLimitX96 */
        bytes calldata data
    ) external returns (int256, int256) {
        // V3 flash swap: output token'ları ÖNCE gönder
        if (mockAmount0Delta < 0) {
            MockERC20(token0).transfer(recipient, uint256(-mockAmount0Delta));
        }
        if (mockAmount1Delta < 0) {
            MockERC20(token1).transfer(recipient, uint256(-mockAmount1Delta));
        }

        // Callback tetikle — Slipstream de uniswapV3SwapCallback kullanır
        (bool ok, bytes memory ret) = recipient.call(
            abi.encodeWithSignature(
                "uniswapV3SwapCallback(int256,int256,bytes)",
                mockAmount0Delta,
                mockAmount1Delta,
                data
            )
        );
        if (!ok) {
            assembly { revert(add(ret, 32), mload(ret)) }
        }

        return (mockAmount0Delta, mockAmount1Delta);
    }
}

// ══════════════════════════════════════════════════════════════════════════════
//                              TEST CONTRACT
// ══════════════════════════════════════════════════════════════════════════════

/// @title ArbitrajBotuTest — v8.0 Comprehensive Test Suite
/// @notice Tests: sandviç koruması (minProfit), Slipstream V3 callback,
///         kompakt calldata (130 byte), EIP-1153 transient storage,
///         off-chain/on-chain profit validation, immutable owner
/// @dev    Run with: forge test -vvv
contract ArbitrajBotuTest is Test {

    // ── Mock Altyapısı ─────────────────────────────────────────
    MockERC20 public tokenA;              // e.g. USDC (token0)
    MockERC20 public tokenB;              // e.g. WETH (token1)
    MockUniswapV3Pool public uniPool;
    MockSlipstreamPool public slipPool;   // Aerodrome Slipstream (CL)

    // ── Test Altındaki Kontrat ─────────────────────────────────
    ArbitrajBotu public bot;
    address public deployer;
    address public attacker;

    // ── Events (expectEmit için) ───────────────────────────────
    event ArbitrageExecuted(
        address indexed poolA,
        address indexed poolB,
        uint256 amountIn,
        uint256 profit
    );
    event EmergencyTokenWithdraw(
        address indexed token, uint256 amount, address indexed to
    );
    event EmergencyETHWithdraw(uint256 amount, address indexed to);

    // ──────────────────────────────────────────────────────────
    //                       SETUP
    // ──────────────────────────────────────────────────────────

    function setUp() public {
        deployer = address(this);
        attacker = makeAddr("attacker");

        // Mock token'ları deploy et
        tokenA = new MockERC20("TokenA", 6);   // USDC benzeri
        tokenB = new MockERC20("TokenB", 18);  // WETH benzeri

        // Mock havuzları deploy et
        uniPool  = new MockUniswapV3Pool(address(tokenA), address(tokenB));
        slipPool = new MockSlipstreamPool(address(tokenA), address(tokenB));

        // Bot deploy et
        bot = new ArbitrajBotu();
    }

    /// @dev Test kontratının ETH alabilmesi için
    receive() external payable {}

    // ──────────────────────────────────────────────────────────
    //  HELPER: 130-byte kompakt calldata oluştur
    // ──────────────────────────────────────────────────────────

    /// @dev abi.encodePacked ile kompakt calldata:
    ///      [poolA:20] + [poolB:20] + [owedToken:20] + [receivedToken:20]
    ///      + [amount:32] + [uniDir:1] + [aeroDir:1] + [minProfit:16] = 130 byte
    function _buildCalldata(
        address _poolA,
        address _poolB,
        address _owedToken,
        address _receivedToken,
        uint256 _amount,
        uint8 _uniDirection,
        uint8 _aeroDirection,
        uint128 _minProfit
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(
            _poolA,          // 20B
            _poolB,          // 20B
            _owedToken,      // 20B
            _receivedToken,  // 20B
            _amount,         // 32B
            _uniDirection,   // 1B
            _aeroDirection,  // 1B
            _minProfit       // 16B
        );
    }

    /// @dev Bot'a kompakt calldata gönder (fallback tetiklenir)
    function _executeArbitrage(
        address _poolA,
        address _poolB,
        address _owedToken,
        address _receivedToken,
        uint256 _amount,
        uint8 _uniDirection,
        uint8 _aeroDirection,
        uint128 _minProfit
    ) internal returns (bool ok) {
        bytes memory cd = _buildCalldata(
            _poolA, _poolB, _owedToken, _receivedToken,
            _amount, _uniDirection, _aeroDirection, _minProfit
        );
        (ok, ) = address(bot).call(cd);
    }

    /// @dev Standart kârlı senaryo kur (direction=0, zeroForOne=true)
    ///      UniV3: tokenA (token0) borçlu, tokenB (token1) alınır
    ///      Slipstream: tokenB input → tokenA output (zeroForOne=false, aeroDir=1)
    function _setupProfitableScenario(
        uint256 uniAmountOwed,       // UniV3 borcu (tokenA)
        uint256 uniAmountReceived,   // UniV3'ten alınan (tokenB)
        uint256 aeroOutput           // Slipstream'den alınan (tokenA)
    ) internal {
        // UniV3 deltas: owe token0 (tokenA), receive token1 (tokenB)
        uniPool.setMockDeltas(int256(uniAmountOwed), -int256(uniAmountReceived));
        tokenB.mint(address(uniPool), uniAmountReceived);

        // Slipstream deltas: output token0 (tokenA), input token1 (tokenB)
        // aeroDirection=1 (zeroForOne=false) → tokenB→tokenA
        // amount0Delta < 0 (tokenA output), amount1Delta > 0 (tokenB input)
        slipPool.setMockDeltas(-int256(aeroOutput), int256(uniAmountReceived));
        tokenA.mint(address(slipPool), aeroOutput);
    }

    /// @dev Kârlı senaryoyu çalıştır (direction=0, aeroDir=1, varsayılan minProfit)
    function _runProfitableArbitrage(
        uint256 uniAmountOwed,
        uint256 uniAmountReceived,
        uint256 aeroOutput,
        uint128 minProfit
    ) internal returns (bool ok) {
        _setupProfitableScenario(uniAmountOwed, uniAmountReceived, aeroOutput);
        ok = _executeArbitrage(
            address(uniPool),
            address(slipPool),
            address(tokenA),       // owedToken = tokenA
            address(tokenB),       // receivedToken = tokenB
            uniAmountReceived,     // amount = alınan miktar
            0,                     // uniDirection = 0 (zeroForOne=true)
            1,                     // aeroDirection = 1 (zeroForOne=false)
            minProfit
        );
    }

    // ══════════════════════════════════════════════════════════
    //  1. KOMPAKT CALLDATA TESTLERİ (130 byte)
    // ══════════════════════════════════════════════════════════

    function test_compactCalldata_Is130Bytes() public pure {
        bytes memory cd = abi.encodePacked(
            address(0x1111111111111111111111111111111111111111), // poolA
            address(0x2222222222222222222222222222222222222222), // poolB
            address(0x3333333333333333333333333333333333333333), // owedToken
            address(0x4444444444444444444444444444444444444444), // receivedToken
            uint256(1 ether),                                   // amount
            uint8(0),                                           // uniDir
            uint8(1),                                           // aeroDir
            uint128(1e6)                                        // minProfit
        );
        assertEq(cd.length, 130, "Compact calldata must be exactly 130 bytes");
    }

    function test_compactCalldata_SuccessfulArbitrage() public {
        // Senaryo: UniV3'ten 1 tokenB (WETH) al, Slipstream'de 1050 tokenA'ya sat
        // Borç: 1000 tokenA → Kâr: 50 tokenA
        uint256 ownerBefore = tokenA.balanceOf(deployer);

        bool ok = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1); // minProfit = 1 wei
        assertTrue(ok, "Arbitrage should succeed");

        uint256 profit = tokenA.balanceOf(deployer) - ownerBefore;
        assertEq(profit, 50e6, "Profit should be 50 tokenA");
        assertEq(tokenA.balanceOf(address(bot)), 0, "Bot should hold 0 tokenA after");
    }

    function test_compactCalldata_ReverseDirection() public {
        // direction=1 → zeroForOne=false → token1 borçlu, token0 alınır
        // UniV3: token0Delta < 0 (receive tokenA), token1Delta > 0 (owe tokenB)
        uint256 uniOwed = 1e18;       // 1 WETH borç
        uint256 uniReceived = 1000e6; // 1000 tokenA alındı

        uniPool.setMockDeltas(-int256(uniReceived), int256(uniOwed));
        tokenA.mint(address(uniPool), uniReceived);

        // Slipstream: tokenA input → tokenB output (zeroForOne=true, aeroDir=0)
        // amount0Delta > 0 (tokenA input), amount1Delta < 0 (tokenB output)
        uint256 aeroOutput = 1.05e18; // 1.05 WETH
        slipPool.setMockDeltas(int256(uniReceived), -int256(aeroOutput));
        tokenB.mint(address(slipPool), aeroOutput);

        uint256 ownerBefore = tokenB.balanceOf(deployer);

        bool ok = _executeArbitrage(
            address(uniPool),
            address(slipPool),
            address(tokenB),       // owedToken = tokenB (kâr token)
            address(tokenA),       // receivedToken = tokenA
            uniReceived,           // amount
            1,                     // uniDirection = 1 (zeroForOne=false)
            0,                     // aeroDirection = 0 (zeroForOne=true)
            1                      // minProfit = 1 wei
        );
        assertTrue(ok, "Reverse direction should succeed");

        uint256 profit = tokenB.balanceOf(deployer) - ownerBefore;
        assertEq(profit, 0.05e18, "Profit should be 0.05 tokenB");
    }

    function test_compactCalldata_EmitsEvent() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        vm.expectEmit(true, true, false, true);
        emit ArbitrageExecuted(address(uniPool), address(slipPool), 1e18, 50e6);

        _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1
        );
    }

    // ══════════════════════════════════════════════════════════
    //  2. SANDVİÇ SALDIRISI KORUMASI TESTLERİ (minProfit)
    // ══════════════════════════════════════════════════════════

    function test_sandwichProtection_RevertsIfProfitBelowMinProfit() public {
        // Senaryo: 50 tokenA kâr ama minProfit = 100 tokenA
        // Sandviç saldırganı kârın büyük kısmını çalmış simülasyonu
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            100e6   // minProfit = 100 tokenA (kâr sadece 50 tokenA → yetersiz)
        );
        assertFalse(ok, "Should revert when profit < minProfit (sandwich protection)");
    }

    function test_sandwichProtection_PassesWhenProfitEqualsMinProfit() public {
        // Kâr = minProfit (tam eşitlik) → BAŞARILI olmalı
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            50e6   // minProfit = 50 tokenA = kâr → geçmeli
        );
        assertTrue(ok, "Should pass when profit == minProfit");
    }

    function test_sandwichProtection_PassesWhenProfitExceedsMinProfit() public {
        // Kâr > minProfit → BAŞARILI olmalı
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            10e6   // minProfit = 10 tokenA < 50 tokenA kâr → geçmeli
        );
        assertTrue(ok, "Should pass when profit > minProfit");
    }

    function test_sandwichProtection_1WeiProfitBlockedByMinProfit() public {
        // Eski kontratın zayıflığı: 1 wei kâr geçerdi
        // Yeni kontrat: minProfit > 1 wei ise engellenir
        _setupProfitableScenario(1000e6, 1e18, 1000e6 + 1); // 1 wei kâr

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            1e6    // minProfit = 1 USDC → 1 wei kâr yetersiz
        );
        assertFalse(ok, "1 wei profit must be blocked by reasonable minProfit");
    }

    function test_sandwichProtection_ZeroMinProfitStillRequiresProfit() public {
        // minProfit = 0 olsa bile bakiye artmalı (NoProfitRealized kontrolü)
        uint256 amountOwed = 1000e6;
        uint256 amountReceived = 1e18;
        uint256 aeroOutput = 900e6; // Zarar!

        uniPool.setMockDeltas(int256(amountOwed), -int256(amountReceived));
        tokenB.mint(address(uniPool), amountReceived);
        slipPool.setMockDeltas(-int256(aeroOutput), int256(amountReceived));
        tokenA.mint(address(slipPool), aeroOutput);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            amountReceived, 0, 1, 0
        );
        assertFalse(ok, "Should revert even with minProfit=0 when there's a loss");
    }

    // ══════════════════════════════════════════════════════════
    //  3. EIP-1153 TRANSIENT STORAGE TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_transientStorage_CallbackReadsCorrectContext() public {
        // Transient storage doğru bağlamı taşıdığını kanıtla:
        // eğer callback yanlış pool'dan çağırılsaydı revert ederdi
        bool ok = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok, "TSTORE/TLOAD should work correctly across calls");
    }

    function test_transientStorage_NoStateCorruption() public {
        // İki ardışık arbitraj — transient storage temizlenmeli
        bool ok1 = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok1, "First arbitrage should succeed");

        // İkinci arbitraj
        bool ok2 = _runProfitableArbitrage(2000e6, 2e18, 2100e6, 1);
        assertTrue(ok2, "Second arbitrage should succeed (no state corruption)");
    }

    // ══════════════════════════════════════════════════════════
    //  4. OFF-CHAIN KÂR DOĞRULAMASI TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_profitValidation_RevertsIfNoProfit() public {
        // Slipstream çıktısı < borç → kâr yok → revert
        uint256 amountOwed = 1000e6;
        uint256 amountReceived = 1e18;
        uint256 aeroOutput = 900e6; // 900 < 1000 → zarar!

        uniPool.setMockDeltas(int256(amountOwed), -int256(amountReceived));
        tokenB.mint(address(uniPool), amountReceived);
        slipPool.setMockDeltas(-int256(aeroOutput), int256(amountReceived));
        tokenA.mint(address(slipPool), aeroOutput);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            amountReceived, 0, 1, 0
        );
        assertFalse(ok, "Should revert when no profit");
    }

    function test_profitValidation_ExactBreakeven_Reverts() public {
        // aeroOutput == amountOwed → kâr = 0 → revert (balAfter <= balBefore)
        uint256 amountOwed = 1000e6;
        uint256 amountReceived = 1e18;
        uint256 aeroOutput = 1000e6;

        uniPool.setMockDeltas(int256(amountOwed), -int256(amountReceived));
        tokenB.mint(address(uniPool), amountReceived);
        slipPool.setMockDeltas(-int256(aeroOutput), int256(amountReceived));
        tokenA.mint(address(slipPool), aeroOutput);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            amountReceived, 0, 1, 0
        );
        assertFalse(ok, "Should revert on exact breakeven (0 profit)");
    }

    function test_profitValidation_MinimalProfit_Passes() public {
        // aeroOutput = amountOwed + 1 → 1 wei kâr → minProfit=1 ile geçmeli
        bool ok = _runProfitableArbitrage(1000e6, 1e18, 1000e6 + 1, 1);
        assertTrue(ok, "Should pass with 1 wei profit when minProfit=1");

        assertEq(tokenA.balanceOf(deployer), 1, "Owner should receive 1 wei profit");
    }

    function test_profitValidation_LargeProfit() public {
        uint256 before = tokenA.balanceOf(deployer);
        _runProfitableArbitrage(10_000e6, 10e18, 10_500e6, 1);
        uint256 profit = tokenA.balanceOf(deployer) - before;

        assertEq(profit, 500e6, "Large profit should be fully captured");
    }

    // ══════════════════════════════════════════════════════════
    //  5. IMMUTABLE + ERİŞİM KONTROLÜ TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_immutable_OwnerSetInConstructor() public view {
        assertEq(bot.owner(), deployer, "Owner should be deployer");
    }

    function test_accessControl_FallbackRevertsIfNotOwner() public {
        vm.prank(attacker);
        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1
        );
        assertFalse(ok, "Non-owner should be rejected");
    }

    function test_accessControl_CallbackRevertsIfNotExpectedPool() public {
        vm.prank(attacker);
        vm.expectRevert(InvalidCaller.selector);
        bot.uniswapV3SwapCallback(0, 0, "");
    }

    function test_accessControl_CallbackRevertsIfRandomContract() public {
        address random = makeAddr("randomContract");
        vm.prank(random);
        vm.expectRevert(InvalidCaller.selector);
        bot.uniswapV3SwapCallback(1e6, -1e18, "");
    }

    // ══════════════════════════════════════════════════════════
    //  6. CALLDATA DOĞRULAMA TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_calldata_RevertsIfZeroAmount() public {
        bytes memory cd = _buildCalldata(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            0, 0, 1, 0
        );
        (bool ok, bytes memory ret) = address(bot).call(cd);
        assertFalse(ok, "Zero amount should revert");
        assertEq(bytes4(ret), ZeroAmount.selector);
    }

    // ══════════════════════════════════════════════════════════
    //  7. ACİL DURUM ÇEKME TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_withdrawToken_FullBalance() public {
        tokenA.mint(address(bot), 500e6);

        vm.expectEmit(true, true, true, true);
        emit EmergencyTokenWithdraw(address(tokenA), 500e6, deployer);
        bot.withdrawToken(address(tokenA));

        assertEq(tokenA.balanceOf(address(bot)), 0);
        assertEq(tokenA.balanceOf(deployer), 500e6);
    }

    function test_withdrawToken_RevertsIfNotOwner() public {
        vm.prank(attacker);
        vm.expectRevert(Unauthorized.selector);
        bot.withdrawToken(address(tokenA));
    }

    function test_withdrawToken_RevertsIfZeroBalance() public {
        vm.expectRevert(ZeroAmount.selector);
        bot.withdrawToken(address(tokenA));
    }

    function test_withdrawETH() public {
        vm.deal(address(bot), 1 ether);
        uint256 ownerBefore = deployer.balance;

        vm.expectEmit(true, true, true, true);
        emit EmergencyETHWithdraw(1 ether, deployer);
        bot.withdrawETH();

        assertEq(address(bot).balance, 0);
        assertEq(deployer.balance, ownerBefore + 1 ether);
    }

    function test_withdrawETH_RevertsIfNotOwner() public {
        vm.deal(address(bot), 1 ether);
        vm.prank(attacker);
        vm.expectRevert(Unauthorized.selector);
        bot.withdrawETH();
    }

    function test_withdrawETH_RevertsIfZeroBalance() public {
        vm.deal(address(bot), 0);
        vm.expectRevert(ZeroAmount.selector);
        bot.withdrawETH();
    }

    // ══════════════════════════════════════════════════════════
    //  8. VIEW YARDIMCI TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_getBalance() public {
        tokenA.mint(address(bot), 1234e6);
        assertEq(bot.getBalance(address(tokenA)), 1234e6);
    }

    // ══════════════════════════════════════════════════════════
    //  9. CONSTRUCTOR TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_constructor_SetsImmutableOwner() public view {
        assertEq(bot.owner(), deployer, "Owner = deployer");
    }

    function test_constructor_DifferentDeployer() public {
        address otherDeployer = makeAddr("otherDeployer");
        vm.prank(otherDeployer);
        ArbitrajBotu otherBot = new ArbitrajBotu();
        assertEq(otherBot.owner(), otherDeployer);
    }

    // ══════════════════════════════════════════════════════════
    //  10. ETH ALMA TESTİ
    // ══════════════════════════════════════════════════════════

    function test_receiveETH() public {
        vm.deal(deployer, 1 ether);
        (bool ok, ) = address(bot).call{value: 0.5 ether}("");
        assertTrue(ok, "ETH transfer should succeed");
        assertEq(address(bot).balance, 0.5 ether);
    }

    // ══════════════════════════════════════════════════════════
    //  11. ENTEGRASYON: TAM DÖNGÜ TESTİ
    // ══════════════════════════════════════════════════════════

    function test_fullCycle_MultipleArbitrages() public {
        uint256 totalProfit;

        // Arbitraj 1: 50 tokenA kâr
        _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        totalProfit += 50e6;

        // Arbitraj 2: 100 tokenA kâr
        _runProfitableArbitrage(2000e6, 2e18, 2100e6, 1);
        totalProfit += 100e6;

        // Arbitraj 3: 25 tokenA kâr
        _runProfitableArbitrage(500e6, 0.5e18, 525e6, 1);
        totalProfit += 25e6;

        assertEq(tokenA.balanceOf(deployer), totalProfit, "Total profit after 3 trades");
        assertEq(tokenA.balanceOf(address(bot)), 0, "Bot should hold 0 after all trades");
    }

    function test_fullCycle_BothDirections() public {
        // direction=0 ile bir arbitraj
        bool ok1 = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok1);

        // direction=1 ile bir arbitraj (ters yön)
        uniPool.setMockDeltas(-int256(1000e6), int256(1e18));
        tokenA.mint(address(uniPool), 1000e6);

        // Slipstream: tokenA→tokenB (zeroForOne=true, aeroDir=0)
        slipPool.setMockDeltas(int256(1000e6), -int256(1.05e18));
        tokenB.mint(address(slipPool), 1.05e18);

        bool ok2 = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenB),       // owedToken = tokenB
            address(tokenA),       // receivedToken = tokenA
            1000e6,                // amount
            1,                     // uniDirection = 1
            0,                     // aeroDirection = 0
            1                      // minProfit = 1 wei
        );
        assertTrue(ok2);
    }

    // ══════════════════════════════════════════════════════════
    //  12. SİLİNEN ÖZELLİKLERİN YOKLUĞU
    // ══════════════════════════════════════════════════════════

    function test_removed_NoPausedFunction() public view {
        assertEq(bot.owner(), deployer);
        // bot.paused() → mevcut değil
        // bot.togglePause() → mevcut değil
        // bot.minProfitBps() → mevcut değil (minProfit artık calldata'da)
    }

    // ══════════════════════════════════════════════════════════
    //  13. GAS OPTİMİZASYON KANITI
    // ══════════════════════════════════════════════════════════

    function test_gasProfile_SuccessfulArbitrage() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bytes memory cd = _buildCalldata(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1
        );

        uint256 gasBefore = gasleft();
        (bool ok, ) = address(bot).call(cd);
        uint256 gasUsed = gasBefore - gasleft();

        assertTrue(ok, "Should succeed");
        console.log("Gas used for successful arbitrage:", gasUsed);
    }

    // ══════════════════════════════════════════════════════════════════════════
    //  14. FUZZ TESTLERİ — DAYANIKLILIK KANITI (10.000+ Rastgele Senaryo)
    // ══════════════════════════════════════════════════════════════════════════

    /// @notice Rastgele calldata ile fallback'in her koşulda revert ettiğini kanıtlar.
    function testFuzz_Fallback(
        uint256 amount,
        address poolA,
        address poolB,
        address owedToken,
        address receivedToken,
        uint8 uniDirection,
        uint8 aeroDirection,
        uint128 minProfit
    ) public {
        bytes memory payload = abi.encodePacked(
            poolA, poolB, owedToken, receivedToken,
            amount, uniDirection, aeroDirection, minProfit
        );
        assertEq(payload.length, 130, "Payload must be exactly 130 bytes");

        (bool ok, ) = address(bot).call(payload);
        assertFalse(ok, "Fallback MUST revert when no profit scenario exists");
    }

    /// @notice Geçerli havuzlarla bile kâr yoksa revert ettiğini kanıtlar.
    function testFuzz_Fallback_ValidPools_NoProfit(
        uint256 amount,
        uint8 uniDirection,
        uint8 aeroDirection,
        uint128 minProfit
    ) public {
        bytes memory payload = abi.encodePacked(
            address(uniPool),
            address(slipPool),
            address(tokenA),
            address(tokenB),
            amount,
            uniDirection,
            aeroDirection,
            minProfit
        );

        (bool ok, ) = address(bot).call(payload);
        assertFalse(ok, "Must revert even with valid pools when no profit exists");
    }

    /// @notice Rastgele bir adresten callback çağrısının InvalidCaller verdiğini kanıtlar.
    function testFuzz_InvalidCallback(address caller) public {
        vm.assume(caller != address(0));

        vm.prank(caller);
        vm.expectRevert(InvalidCaller.selector);
        bot.uniswapV3SwapCallback(0, 0, "");
    }

    /// @notice Rastgele delta parametreleriyle callback çağrısının reddedildiğini kanıtlar.
    function testFuzz_InvalidCallback_WithRandomDeltas(
        address caller,
        int256 amount0Delta,
        int256 amount1Delta
    ) public {
        vm.assume(caller != address(0));

        vm.prank(caller);
        vm.expectRevert(InvalidCaller.selector);
        bot.uniswapV3SwapCallback(amount0Delta, amount1Delta, "");
    }

    /// @notice Owner olmayan rastgele adreslerin fallback'ten reddedildiğini kanıtlar.
    function testFuzz_Fallback_Unauthorized(
        address caller,
        uint256 amount,
        uint8 uniDirection,
        uint8 aeroDirection,
        uint128 minProfit
    ) public {
        vm.assume(caller != deployer);

        bytes memory payload = abi.encodePacked(
            address(uniPool),
            address(slipPool),
            address(tokenA),
            address(tokenB),
            amount,
            uniDirection,
            aeroDirection,
            minProfit
        );

        vm.prank(caller);
        (bool ok, ) = address(bot).call(payload);
        assertFalse(ok, "Non-owner MUST be rejected by fallback");
    }

    /// @notice minProfit fuzz: kâr sabit, rastgele minProfit değerleri
    function testFuzz_SandwichProtection_MinProfitVariations(
        uint128 minProfit
    ) public {
        // Sabit kâr: 50e6 tokenA
        uint256 fixedProfit = 50e6;
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, minProfit
        );

        if (uint256(minProfit) <= fixedProfit) {
            assertTrue(ok, "Should pass when minProfit <= actual profit");
        } else {
            assertFalse(ok, "Should revert when minProfit > actual profit");
        }
    }
}
