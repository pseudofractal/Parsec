module DemoModule

const PI_APPROX = 3.14159

abstract type Shape end

struct Circle <: Shape
    radius::Float64
end

primitive type MyBits 32 end

typealias ShapeAlias Shape

macro mymacro(expr)
    :(println("Macro says: ", $expr))
end

function area(c::Circle)
    return π * c.radius^2
end

square(x) = x * x

baremodule SubModule
    const VALUE = 42

    struct Point
        x::Int
        y::Int
    end

    function move(p::Point, dx, dy)
        Point(p.x + dx, p.y + dy)
    end
end

end # module
