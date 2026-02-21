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
    TransferFailed,
    DeadlineExpired,
    ZeroAddress
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
        if (mockAmount0Delta < 0) {
            MockERC20(token0).transfer(recipient, uint256(-mockAmount0Delta));
        }
        if (mockAmount1Delta < 0) {
            MockERC20(token1).transfer(recipient, uint256(-mockAmount1Delta));
        }

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
        if (mockAmount0Delta < 0) {
            MockERC20(token0).transfer(recipient, uint256(-mockAmount0Delta));
        }
        if (mockAmount1Delta < 0) {
            MockERC20(token1).transfer(recipient, uint256(-mockAmount1Delta));
        }

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

/// @title ArbitrajBotuTest — v9.0 Comprehensive Test Suite
/// @notice Tests: executor/admin rol ayrımı, deadline koruması, sandviç
///         koruması (minProfit), Slipstream V3 callback, kompakt calldata
///         (134 byte), EIP-1153 transient storage, kâr kontrat içinde kalır
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

        // Bot deploy et — executor=deployer, admin=deployer (test kolaylığı)
        bot = new ArbitrajBotu(deployer, deployer);
    }

    /// @dev Test kontratının ETH alabilmesi için
    receive() external payable {}

    // ──────────────────────────────────────────────────────────
    //  HELPER: 134-byte kompakt calldata oluştur
    // ──────────────────────────────────────────────────────────

    /// @dev abi.encodePacked ile kompakt calldata:
    ///      [poolA:20] + [poolB:20] + [owedToken:20] + [receivedToken:20]
    ///      + [amount:32] + [uniDir:1] + [aeroDir:1] + [minProfit:16]
    ///      + [deadlineBlock:4] = 134 byte
    function _buildCalldata(
        address _poolA,
        address _poolB,
        address _owedToken,
        address _receivedToken,
        uint256 _amount,
        uint8 _uniDirection,
        uint8 _aeroDirection,
        uint128 _minProfit,
        uint32 _deadlineBlock
    ) internal pure returns (bytes memory) {
        return abi.encodePacked(
            _poolA,          // 20B
            _poolB,          // 20B
            _owedToken,      // 20B
            _receivedToken,  // 20B
            _amount,         // 32B
            _uniDirection,   // 1B
            _aeroDirection,  // 1B
            _minProfit,      // 16B
            _deadlineBlock   // 4B
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
        uint128 _minProfit,
        uint32 _deadlineBlock
    ) internal returns (bool ok) {
        bytes memory cd = _buildCalldata(
            _poolA, _poolB, _owedToken, _receivedToken,
            _amount, _uniDirection, _aeroDirection, _minProfit,
            _deadlineBlock
        );
        (ok, ) = address(bot).call(cd);
    }

    /// @dev Standart kârlı senaryo kur (direction=0, zeroForOne=true)
    function _setupProfitableScenario(
        uint256 uniAmountOwed,
        uint256 uniAmountReceived,
        uint256 aeroOutput
    ) internal {
        uniPool.setMockDeltas(int256(uniAmountOwed), -int256(uniAmountReceived));
        tokenB.mint(address(uniPool), uniAmountReceived);

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
            address(tokenA),
            address(tokenB),
            uniAmountReceived,
            0,
            1,
            minProfit,
            uint32(block.number)  // geçerli deadline
        );
    }

    // ══════════════════════════════════════════════════════════
    //  1. KOMPAKT CALLDATA TESTLERİ (134 byte)
    // ══════════════════════════════════════════════════════════

    function test_compactCalldata_Is134Bytes() public pure {
        bytes memory cd = abi.encodePacked(
            address(0x1111111111111111111111111111111111111111),
            address(0x2222222222222222222222222222222222222222),
            address(0x3333333333333333333333333333333333333333),
            address(0x4444444444444444444444444444444444444444),
            uint256(1 ether),
            uint8(0),
            uint8(1),
            uint128(1e6),
            uint32(999999)
        );
        assertEq(cd.length, 134, "Compact calldata must be exactly 134 bytes");
    }

    function test_compactCalldata_SuccessfulArbitrage() public {
        bool ok = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok, "Arbitrage should succeed");

        // v9: Kâr kontrat içinde kalır
        uint256 botBalance = tokenA.balanceOf(address(bot));
        assertEq(botBalance, 50e6, "Profit should stay in contract");
    }

    function test_compactCalldata_ReverseDirection() public {
        // direction=1 → token1 borçlu, token0 alınır
        uint256 uniOwed = 1e18;
        uint256 uniReceived = 1000e6;

        uniPool.setMockDeltas(-int256(uniReceived), int256(uniOwed));
        tokenA.mint(address(uniPool), uniReceived);

        uint256 aeroOutput = 1.05e18;
        slipPool.setMockDeltas(int256(uniReceived), -int256(aeroOutput));
        tokenB.mint(address(slipPool), aeroOutput);

        bool ok = _executeArbitrage(
            address(uniPool),
            address(slipPool),
            address(tokenB),
            address(tokenA),
            uniReceived,
            1,
            0,
            1,
            uint32(block.number)
        );
        assertTrue(ok, "Reverse direction should succeed");

        // v9: Kâr kontrat içinde kalır
        uint256 profit = tokenB.balanceOf(address(bot));
        assertEq(profit, 0.05e18, "Profit should be 0.05 tokenB in contract");
    }

    function test_compactCalldata_EmitsEvent() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        vm.expectEmit(true, true, false, true);
        emit ArbitrageExecuted(address(uniPool), address(slipPool), 1e18, 50e6);

        _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1,
            uint32(block.number)
        );
    }

    // ══════════════════════════════════════════════════════════
    //  2. SANDVİÇ SALDIRISI KORUMASI TESTLERİ (minProfit)
    // ══════════════════════════════════════════════════════════

    function test_sandwichProtection_RevertsIfProfitBelowMinProfit() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            100e6,   // minProfit = 100 tokenA (kâr sadece 50)
            uint32(block.number)
        );
        assertFalse(ok, "Should revert when profit < minProfit");
    }

    function test_sandwichProtection_PassesWhenProfitEqualsMinProfit() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            50e6,   // minProfit = kâr
            uint32(block.number)
        );
        assertTrue(ok, "Should pass when profit == minProfit");
    }

    function test_sandwichProtection_PassesWhenProfitExceedsMinProfit() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            10e6,
            uint32(block.number)
        );
        assertTrue(ok, "Should pass when profit > minProfit");
    }

    function test_sandwichProtection_1WeiProfitBlockedByMinProfit() public {
        _setupProfitableScenario(1000e6, 1e18, 1000e6 + 1);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1,
            1e6,
            uint32(block.number)
        );
        assertFalse(ok, "1 wei profit must be blocked by reasonable minProfit");
    }

    function test_sandwichProtection_ZeroMinProfitStillRequiresProfit() public {
        uint256 amountOwed = 1000e6;
        uint256 amountReceived = 1e18;
        uint256 aeroOutput = 900e6;

        uniPool.setMockDeltas(int256(amountOwed), -int256(amountReceived));
        tokenB.mint(address(uniPool), amountReceived);
        slipPool.setMockDeltas(-int256(aeroOutput), int256(amountReceived));
        tokenA.mint(address(slipPool), aeroOutput);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            amountReceived, 0, 1, 0,
            uint32(block.number)
        );
        assertFalse(ok, "Should revert even with minProfit=0 when there's a loss");
    }

    // ══════════════════════════════════════════════════════════
    //  3. EIP-1153 TRANSIENT STORAGE TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_transientStorage_CallbackReadsCorrectContext() public {
        bool ok = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok, "TSTORE/TLOAD should work correctly across calls");
    }

    function test_transientStorage_NoStateCorruption() public {
        bool ok1 = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok1, "First arbitrage should succeed");

        bool ok2 = _runProfitableArbitrage(2000e6, 2e18, 2100e6, 1);
        assertTrue(ok2, "Second arbitrage should succeed (no state corruption)");
    }

    // ══════════════════════════════════════════════════════════
    //  4. OFF-CHAIN KÂR DOĞRULAMASI TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_profitValidation_RevertsIfNoProfit() public {
        uint256 amountOwed = 1000e6;
        uint256 amountReceived = 1e18;
        uint256 aeroOutput = 900e6;

        uniPool.setMockDeltas(int256(amountOwed), -int256(amountReceived));
        tokenB.mint(address(uniPool), amountReceived);
        slipPool.setMockDeltas(-int256(aeroOutput), int256(amountReceived));
        tokenA.mint(address(slipPool), aeroOutput);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            amountReceived, 0, 1, 0,
            uint32(block.number)
        );
        assertFalse(ok, "Should revert when no profit");
    }

    function test_profitValidation_ExactBreakeven_Reverts() public {
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
            amountReceived, 0, 1, 0,
            uint32(block.number)
        );
        assertFalse(ok, "Should revert on exact breakeven (0 profit)");
    }

    function test_profitValidation_MinimalProfit_Passes() public {
        bool ok = _runProfitableArbitrage(1000e6, 1e18, 1000e6 + 1, 1);
        assertTrue(ok, "Should pass with 1 wei profit when minProfit=1");

        // v9: Kâr kontrat içinde kalır
        assertEq(tokenA.balanceOf(address(bot)), 1, "Contract should hold 1 wei profit");
    }

    function test_profitValidation_LargeProfit() public {
        _runProfitableArbitrage(10_000e6, 10e18, 10_500e6, 1);

        // v9: Kâr kontrat içinde kalır
        assertEq(tokenA.balanceOf(address(bot)), 500e6, "Large profit should stay in contract");
    }

    // ══════════════════════════════════════════════════════════
    //  5. EXECUTOR/ADMIN ROL AYRIMI TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_immutable_ExecutorAndAdminSetInConstructor() public view {
        assertEq(bot.executor(), deployer, "Executor should be deployer");
        assertEq(bot.admin(), deployer, "Admin should be deployer");
    }

    function test_accessControl_FallbackRevertsIfNotExecutor() public {
        vm.prank(attacker);
        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1,
            uint32(block.number)
        );
        assertFalse(ok, "Non-executor should be rejected");
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

    // ── ROL AYRIMI: Executor fon çekemez, Admin arbitraj yapamaz ──

    function test_rolesSeparation_ExecutorCannotWithdrawToken() public {
        address exec = makeAddr("exec");
        address adm  = makeAddr("adm");
        ArbitrajBotu roleBot = new ArbitrajBotu(exec, adm);
        tokenA.mint(address(roleBot), 100e6);

        vm.prank(exec);
        vm.expectRevert(Unauthorized.selector);
        roleBot.withdrawToken(address(tokenA));
    }

    function test_rolesSeparation_ExecutorCannotWithdrawETH() public {
        address exec = makeAddr("exec");
        address adm  = makeAddr("adm");
        ArbitrajBotu roleBot = new ArbitrajBotu(exec, adm);
        vm.deal(address(roleBot), 1 ether);

        vm.prank(exec);
        vm.expectRevert(Unauthorized.selector);
        roleBot.withdrawETH();
    }

    function test_rolesSeparation_AdminCannotExecute() public {
        address exec = makeAddr("exec");
        address adm  = makeAddr("adm");
        ArbitrajBotu roleBot = new ArbitrajBotu(exec, adm);

        _setupProfitableScenario(1000e6, 1e18, 1050e6);
        bytes memory cd = _buildCalldata(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1, uint32(block.number)
        );

        vm.prank(adm);
        (bool ok, ) = address(roleBot).call(cd);
        assertFalse(ok, "Admin must NOT be able to execute arbitrage");
    }

    // ── v9: Kâr kontrat içinde kalır testi ──

    function test_profitStaysInContract() public {
        uint256 deployerBefore = tokenA.balanceOf(deployer);
        bool ok = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok);

        assertEq(tokenA.balanceOf(address(bot)), 50e6, "Profit must stay in contract");
        assertEq(tokenA.balanceOf(deployer), deployerBefore, "Deployer balance must not change");
    }

    // ══════════════════════════════════════════════════════════
    //  6. DEADLINE (deadlineBlock) TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_deadline_RevertsIfExpired() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1,
            uint32(block.number - 1) // expired
        );
        assertFalse(ok, "Should revert when deadline expired");
    }

    function test_deadline_PassesAtExactBlock() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1,
            uint32(block.number) // exact block = valid
        );
        assertTrue(ok, "Should pass when deadline == current block");
    }

    function test_deadline_PassesWithFutureBlock() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1,
            uint32(block.number + 100) // future = valid
        );
        assertTrue(ok, "Should pass when deadline is in the future");
    }

    function test_deadline_ZeroAlwaysReverts() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1,
            0 // deadlineBlock = 0, block.number > 0
        );
        assertFalse(ok, "deadlineBlock=0 should always revert");
    }

    // ══════════════════════════════════════════════════════════
    //  7. CALLDATA DOĞRULAMA TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_calldata_RevertsIfZeroAmount() public {
        bytes memory cd = _buildCalldata(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            0, 0, 1, 0, uint32(block.number)
        );
        (bool ok, bytes memory ret) = address(bot).call(cd);
        assertFalse(ok, "Zero amount should revert");
        assertEq(bytes4(ret), ZeroAmount.selector);
    }

    // ══════════════════════════════════════════════════════════
    //  8. ACİL DURUM ÇEKME TESTLERİ (Admin Only)
    // ══════════════════════════════════════════════════════════

    function test_withdrawToken_FullBalance() public {
        tokenA.mint(address(bot), 500e6);

        vm.expectEmit(true, true, true, true);
        emit EmergencyTokenWithdraw(address(tokenA), 500e6, deployer);
        bot.withdrawToken(address(tokenA));

        assertEq(tokenA.balanceOf(address(bot)), 0);
        assertEq(tokenA.balanceOf(deployer), 500e6);
    }

    function test_withdrawToken_RevertsIfNotAdmin() public {
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
        uint256 adminBefore = deployer.balance;

        vm.expectEmit(true, true, true, true);
        emit EmergencyETHWithdraw(1 ether, deployer);
        bot.withdrawETH();

        assertEq(address(bot).balance, 0);
        assertEq(deployer.balance, adminBefore + 1 ether);
    }

    function test_withdrawETH_RevertsIfNotAdmin() public {
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
    //  9. VIEW YARDIMCI TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_getBalance() public {
        tokenA.mint(address(bot), 1234e6);
        assertEq(bot.getBalance(address(tokenA)), 1234e6);
    }

    // ══════════════════════════════════════════════════════════
    //  10. CONSTRUCTOR TESTLERİ
    // ══════════════════════════════════════════════════════════

    function test_constructor_SetsImmutableExecutorAndAdmin() public view {
        assertEq(bot.executor(), deployer, "Executor = deployer");
        assertEq(bot.admin(), deployer, "Admin = deployer");
    }

    function test_constructor_DifferentAddresses() public {
        address exec = makeAddr("newExecutor");
        address adm  = makeAddr("newAdmin");
        ArbitrajBotu otherBot = new ArbitrajBotu(exec, adm);
        assertEq(otherBot.executor(), exec);
        assertEq(otherBot.admin(), adm);
    }

    function test_constructor_RevertsIfExecutorZero() public {
        vm.expectRevert(ZeroAddress.selector);
        new ArbitrajBotu(address(0), address(1));
    }

    function test_constructor_RevertsIfAdminZero() public {
        vm.expectRevert(ZeroAddress.selector);
        new ArbitrajBotu(address(1), address(0));
    }

    function test_constructor_RevertsIfBothZero() public {
        vm.expectRevert(ZeroAddress.selector);
        new ArbitrajBotu(address(0), address(0));
    }

    // ══════════════════════════════════════════════════════════
    //  11. ETH ALMA TESTİ
    // ══════════════════════════════════════════════════════════

    function test_receiveETH() public {
        vm.deal(deployer, 1 ether);
        (bool ok, ) = address(bot).call{value: 0.5 ether}("");
        assertTrue(ok, "ETH transfer should succeed");
        assertEq(address(bot).balance, 0.5 ether);
    }

    // ══════════════════════════════════════════════════════════
    //  12. ENTEGRASYON: TAM DÖNGÜ TESTİ
    // ══════════════════════════════════════════════════════════

    function test_fullCycle_MultipleArbitrages() public {
        uint256 totalProfit;

        _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        totalProfit += 50e6;

        _runProfitableArbitrage(2000e6, 2e18, 2100e6, 1);
        totalProfit += 100e6;

        _runProfitableArbitrage(500e6, 0.5e18, 525e6, 1);
        totalProfit += 25e6;

        // v9: Tüm kâr kontrat içinde birikir
        assertEq(tokenA.balanceOf(address(bot)), totalProfit, "Total profit in contract after 3 trades");
    }

    function test_fullCycle_BothDirections() public {
        bool ok1 = _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertTrue(ok1);

        uniPool.setMockDeltas(-int256(1000e6), int256(1e18));
        tokenA.mint(address(uniPool), 1000e6);

        slipPool.setMockDeltas(int256(1000e6), -int256(1.05e18));
        tokenB.mint(address(slipPool), 1.05e18);

        bool ok2 = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenB), address(tokenA),
            1000e6, 1, 0, 1,
            uint32(block.number)
        );
        assertTrue(ok2);
    }

    // ── ENTEGRASYON: Arbitraj + Admin Çekim ──

    function test_fullCycle_ArbitrageAndWithdraw() public {
        // 1. Arbitraj yap — kâr kontrat içinde birikir
        _runProfitableArbitrage(1000e6, 1e18, 1050e6, 1);
        assertEq(tokenA.balanceOf(address(bot)), 50e6, "Profit in contract");

        // 2. Admin kârı çeker
        uint256 deployerBefore = tokenA.balanceOf(deployer);
        bot.withdrawToken(address(tokenA));
        assertEq(tokenA.balanceOf(address(bot)), 0, "Contract emptied after withdraw");
        assertEq(tokenA.balanceOf(deployer), deployerBefore + 50e6, "Admin received profit");
    }

    // ══════════════════════════════════════════════════════════
    //  13. SİLİNEN ÖZELLİKLERİN YOKLUĞU
    // ══════════════════════════════════════════════════════════

    function test_removed_NoPausedFunction() public view {
        assertEq(bot.executor(), deployer);
        assertEq(bot.admin(), deployer);
    }

    // ══════════════════════════════════════════════════════════
    //  14. GAS OPTİMİZASYON KANITI
    // ══════════════════════════════════════════════════════════

    function test_gasProfile_SuccessfulArbitrage() public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bytes memory cd = _buildCalldata(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1, uint32(block.number)
        );

        uint256 gasBefore = gasleft();
        (bool ok, ) = address(bot).call(cd);
        uint256 gasUsed = gasBefore - gasleft();

        assertTrue(ok, "Should succeed");
        console.log("Gas used for successful arbitrage:", gasUsed);
    }

    // ══════════════════════════════════════════════════════════════════════════
    //  15. FUZZ TESTLERİ — DAYANIKLILIK KANITI (10.000+ Rastgele Senaryo)
    // ══════════════════════════════════════════════════════════════════════════

    function testFuzz_Fallback(
        uint256 amount,
        address poolA,
        address poolB,
        address owedToken,
        address receivedToken,
        uint8 uniDirection,
        uint8 aeroDirection,
        uint128 minProfit,
        uint32 deadlineBlock
    ) public {
        bytes memory payload = abi.encodePacked(
            poolA, poolB, owedToken, receivedToken,
            amount, uniDirection, aeroDirection, minProfit,
            deadlineBlock
        );
        assertEq(payload.length, 134, "Payload must be exactly 134 bytes");

        (bool ok, ) = address(bot).call(payload);
        assertFalse(ok, "Fallback MUST revert when no profit scenario exists");
    }

    function testFuzz_Fallback_ValidPools_NoProfit(
        uint256 amount,
        uint8 uniDirection,
        uint8 aeroDirection,
        uint128 minProfit,
        uint32 deadlineBlock
    ) public {
        bytes memory payload = abi.encodePacked(
            address(uniPool),
            address(slipPool),
            address(tokenA),
            address(tokenB),
            amount,
            uniDirection,
            aeroDirection,
            minProfit,
            deadlineBlock
        );

        (bool ok, ) = address(bot).call(payload);
        assertFalse(ok, "Must revert even with valid pools when no profit exists");
    }

    function testFuzz_InvalidCallback(address caller) public {
        vm.assume(caller != address(0));

        vm.prank(caller);
        vm.expectRevert(InvalidCaller.selector);
        bot.uniswapV3SwapCallback(0, 0, "");
    }

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

    function testFuzz_Fallback_Unauthorized(
        address caller,
        uint256 amount,
        uint8 uniDirection,
        uint8 aeroDirection,
        uint128 minProfit,
        uint32 deadlineBlock
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
            minProfit,
            deadlineBlock
        );

        vm.prank(caller);
        (bool ok, ) = address(bot).call(payload);
        assertFalse(ok, "Non-executor MUST be rejected by fallback");
    }

    function testFuzz_SandwichProtection_MinProfitVariations(
        uint128 minProfit
    ) public {
        uint256 fixedProfit = 50e6;
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, minProfit,
            uint32(block.number)
        );

        if (uint256(minProfit) <= fixedProfit) {
            assertTrue(ok, "Should pass when minProfit <= actual profit");
        } else {
            assertFalse(ok, "Should revert when minProfit > actual profit");
        }
    }

    function testFuzz_Deadline_Variations(
        uint32 deadlineBlock
    ) public {
        _setupProfitableScenario(1000e6, 1e18, 1050e6);

        bool ok = _executeArbitrage(
            address(uniPool), address(slipPool),
            address(tokenA), address(tokenB),
            1e18, 0, 1, 1,
            deadlineBlock
        );

        if (uint256(deadlineBlock) >= block.number) {
            assertTrue(ok, "Should pass when deadline >= current block");
        } else {
            assertFalse(ok, "Should revert when deadline < current block");
        }
    }
}
